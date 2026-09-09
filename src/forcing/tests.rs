use super::*;
use crate::board::{bit, idx};
use crate::movegen::Move;
fn job(objective: Objective) -> Job {
    Job {
        progress: Mutex::new(Progress {
            rules: Rules::default(),
            objective,
            first: true,
            completed_depth: 0,
            proved_depth: None,
            avoided: false,
        }),
        table: Arc::new(Table::new(1024 * 1024)),
        implies: None,
        stop: Arc::new(AtomicBool::new(false)),
        nodes: AtomicU64::new(0),
        name: "test".into(),
        deadline: None,
        blocked: None,
    }
}
fn brute(pos: Position, turn: bool, depth: u16, obj: Objective) -> bool {
    if depth == 0 {
        return false;
    }
    let moves = generate_moves(&pos);
    if moves.is_empty() {
        return false;
    }
    let mut result = !turn;
    for mv in moves {
        let a = apply_move(&pos, mv, &Rules::default());
        let v = obj
            .terminal(&a, turn)
            .unwrap_or_else(|| brute(a.next, !turn, depth - 1, obj));
        if v == turn {
            result = v;
            break;
        }
    }
    result
}
#[test]
fn objective_terminals_distinguish_clean_and_double_on_either_turn() {
    let p = Position {
        us: bit(idx(0, 0)) | bit(idx(1, 0)),
        them: bit(idx(2, 1)) | bit(idx(2, 3)) | bit(idx(2, 4)),
    };
    let a = apply_move(&p, Move(idx(2, 0)), &Rules::default());
    assert!(a.simultaneous_lines);
    for turn in [false, true] {
        assert_eq!(Objective::Clean.terminal(&a, turn), Some(false));
        assert_eq!(Objective::DoubleOnly.terminal(&a, turn), Some(true));
        assert_eq!(Objective::CleanOrDouble.terminal(&a, turn), Some(true));
    }
    let clean = apply_move(
        &Position { us: p.us, them: 0 },
        Move(idx(2, 0)),
        &Rules::default(),
    );
    assert_eq!(Objective::Clean.terminal(&clean, true), Some(true));
    assert_eq!(Objective::Clean.terminal(&clean, false), Some(false));
    assert_eq!(Objective::DoubleOnly.terminal(&clean, true), Some(false));

    let eight = Position {
        us: [
            idx(0, 0),
            idx(0, 2),
            idx(0, 4),
            idx(2, 0),
            idx(2, 2),
            idx(2, 4),
            idx(4, 0),
        ]
        .iter()
        .fold(0, |bb, &sq| bb | bit(sq)),
        them: 0,
    };
    let all = apply_move(&eight, Move(idx(4, 2)), &Rules::default());
    assert_eq!(all.next.them.count_ones(), 8);
    assert!(!crate::board::has_three_in_row(all.next.them));
    assert_eq!(Objective::Clean.terminal(&all, true), Some(true));
    assert_eq!(Objective::DoubleOnly.terminal(&all, true), Some(false));
    let mixed = Position {
        them: bit(idx(4, 3)) | bit(idx(3, 4)) | bit(idx(5, 4)),
        ..eight
    };
    let mixed = apply_move(&mixed, Move(idx(4, 2)), &Rules::default());
    assert_eq!(mixed.outcome, MoveOutcome::Draw);
    assert!(!mixed.simultaneous_lines);
    assert_eq!(Objective::CleanOrDouble.terminal(&mixed, true), Some(false));
}
#[test]
fn search_matches_uncached_tree_for_both_roles_and_objectives() {
    let fixtures = [
        Position::start(),
        Position {
            us: bit(0) | bit(6),
            them: bit(13) | bit(15) | bit(16),
        },
        Position {
            us: bit(7) | bit(22),
            them: bit(8) | bit(21),
        },
    ];
    for obj in [
        Objective::Clean,
        Objective::CleanOrDouble,
        Objective::DoubleOnly,
    ] {
        let job = job(obj);
        for p in fixtures {
            for turn in [false, true] {
                for depth in [3, 1, 2, 3] {
                    let expected = brute(p, turn, depth, obj);
                    assert_eq!(
                        job.solve(
                            p.canonical_key(),
                            turn,
                            depth,
                            &Rules::default(),
                            obj,
                            false
                        )
                        .unwrap(),
                        expected,
                        "{obj:?} {turn} {depth} {p:?}"
                    );
                    assert_eq!(
                        job.solve(p.canonical_key(), turn, depth, &Rules::default(), obj, true)
                            .unwrap(),
                        expected
                    );
                }
            }
        }
    }
}
#[test]
fn implications_transfer_only_the_valid_bound_and_share_roles() {
    let mut clean = job(Objective::Clean);
    let mut either = job(Objective::CleanOrDouble);
    clean.implies = Some((true, either.table.clone()));
    either.implies = Some((false, clean.table.clone()));
    for turn in [false, true] {
        let key = code(0, turn);
        clean.store(key, 2, false, 0);
        assert!(either.table.get(key).is_none());
        either.store(key, 8, true, 1);
        assert_eq!(clean.table.get(key).unwrap().good, 0);
        clean.merge(Entry {
            code: key,
            bad: 4,
            good: 6,
            best: 2,
        });
        let e = either.table.get(key).unwrap();
        assert_eq!((e.bad, e.good), (0, 6));
        either.merge(Entry {
            code: key,
            bad: 5,
            good: 6,
            best: 3,
        });
        let e = clean.table.get(key).unwrap();
        assert_eq!((e.bad, e.good), (5, 6));
    }
    let mut second = job(Objective::Clean);
    second.progress.lock().first = false;
    second.table = clean.table.clone();
    assert!(Arc::ptr_eq(&clean.table, &second.table));
    second.store(code(1, false), 3, false, 0);
    assert_eq!(clean.table.get(code(1, false)).unwrap().bad, 3);
}

#[test]
fn shared_search_matches_reachable_trees_with_eviction() {
    let mut clean = job(Objective::Clean);
    let mut either = job(Objective::CleanOrDouble);
    clean.table = Arc::new(Table::new(0));
    either.table = Arc::new(Table::new(0));
    clean.implies = Some((true, either.table.clone()));
    either.implies = Some((false, clean.table.clone()));
    let mut seed = 7351u64;
    let mut pos = Position::start();
    for sample in 0..16 {
        let moves = generate_moves(&pos);
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let applied = apply_move(&pos, moves[seed as usize % moves.len()], &Rules::default());
        pos = if applied.outcome == MoveOutcome::Ongoing {
            applied.next
        } else {
            Position::start()
        };
        for turn in [false, true] {
            for depth in [1, 3, 2, 4] {
                for j in [&either, &clean] {
                    let obj = j.progress.lock().objective;
                    let expected = brute(pos, turn, depth, obj);
                    assert_eq!(
                        j.solve(
                            pos.canonical_key(),
                            turn,
                            depth,
                            &Rules::default(),
                            obj,
                            sample % 2 == 0
                        )
                        .unwrap(),
                        expected,
                        "sample={sample} {obj:?} turn={turn} depth={depth}"
                    );
                }
            }
        }
    }
}

#[test]
fn compact_certificates_match_nontrivial_uncached_wins() {
    let dir = std::env::temp_dir().join(format!("gekitai-compact-trees-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let mut seed = 351u64;
    let mut checked = 0;
    for _ in 0..2000 {
        let mut pos = Position::start();
        for sq in 0..36 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            match (seed >> 32) % 12 {
                0 | 1 if pos.us_count() < 6 => pos.us |= bit(sq),
                2 | 3 if pos.them_count() < 6 => pos.them |= bit(sq),
                _ => {}
            }
        }
        if crate::board::has_three_in_row(pos.us) || crate::board::has_three_in_row(pos.them) {
            continue;
        }
        let obj = if checked % 2 == 0 {
            Objective::Clean
        } else {
            Objective::CleanOrDouble
        };
        if brute(pos, true, 1, obj) || !brute(pos, true, 3, obj) {
            continue;
        }
        let j = job(obj);
        let p = j.progress.lock().clone();
        let key = pos.canonical_key();
        assert!(j.solve(key, true, 3, &p.rules, obj, false).unwrap());
        let header = Header {
            start_key: key,
            ..j.certificate_header(&p, Some(3))
        };
        let cert = j
            .positive_certificate(&p, 3, &dir, Some(Work::new(header)), 10000)
            .unwrap()
            .unwrap();
        cert.verify().unwrap();
        assert!(cert.nodes.len() > 1);
        if obj == Objective::Clean {
            let quick = tactical_certificate(
                pos,
                p.rules,
                Instant::now() + Duration::from_secs(1),
                Arc::new(AtomicBool::new(false)),
                std::collections::HashSet::new(),
            )
            .unwrap();
            quick.verify().unwrap();
            assert_eq!(quick.header.forced_depth, Some(3));
            // All ongoing root choices would repeat a prior game position.
            // The interactive search must not claim its history-free win.
            let blocked = generate_moves(&pos)
                .into_iter()
                .filter_map(|mv| {
                    let a = apply_move(&pos, mv, &p.rules);
                    (a.outcome == MoveOutcome::Ongoing).then(|| code(a.next.canonical_key(), false))
                })
                .collect();
            assert!(tactical_certificate(
                pos,
                p.rules,
                Instant::now() + Duration::from_secs(1),
                Arc::new(AtomicBool::new(false)),
                blocked
            )
            .is_none());
        }
        checked += 1;
        if checked == 4 {
            break;
        }
    }
    assert_eq!(
        checked, 4,
        "deterministic fixtures must exercise nonterminal choices"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn compact_certificate_survives_budget_pause_and_resume() {
    let j = job(Objective::Clean);
    let mut p = j.progress.lock().clone();
    p.first = false;
    let key = Position {
        us: 0,
        them: bit(0) | bit(1) | bit(28) | bit(29),
    }
    .canonical_key();
    let header = Header {
        start_key: key,
        ..j.certificate_header(&p, Some(2))
    };
    let dir = std::env::temp_dir().join(format!("gekitai-compact-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    assert!(j
        .positive_certificate(&p, 2, &dir, Some(Work::new(header)), 0)
        .unwrap()
        .is_none());
    let saved = Work::load(&dir.join("test.certificate-work")).unwrap();
    assert_eq!(saved.pending, vec![(code(key, false), 2)]);
    assert_eq!(saved.certificate.nodes.len(), 0);
    assert!(!j.progress.lock().avoided && j.progress.lock().proved_depth.is_none());
    let certificate = j
        .positive_certificate(&p, 2, &dir, Some(saved), 1)
        .unwrap()
        .unwrap();
    assert_eq!(certificate.nodes.len(), 1);
    certificate.verify().unwrap();
    let saved = Work::load(&dir.join("test.certificate-work")).unwrap();
    assert!(saved.pending.is_empty());
    assert!(saved.implicit > 0);
    // A completed work file still needs verification after restart.
    let replay = j
        .positive_certificate(&p, 2, &dir, Some(saved), 1)
        .unwrap()
        .unwrap();
    replay.verify().unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checkpoint_roundtrip_rejects_wrong_objective_and_truncation() {
    let dir = std::env::temp_dir().join(format!("gekitai-forcing-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let j = job(Objective::Clean);
    j.store(code(0, true), 3, false, 0);
    j.store(code(0, true), 5, true, 1);
    j.save(&dir, 1024 * 1024).unwrap();
    let loaded = job(Objective::Clean);
    loaded.load(&dir).unwrap();
    let e = loaded.table.get(code(0, true)).unwrap();
    assert_eq!((e.bad, e.good), (3, 5));
    assert!(job(Objective::DoubleOnly).load(&dir).is_err());
    let f = File::options()
        .write(true)
        .open(dir.join("test.ckpt"))
        .unwrap();
    f.set_len(f.metadata().unwrap().len() - 1).unwrap();
    assert!(job(Objective::Clean).load(&dir).is_err());
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn canceled_search_does_not_cache_an_answer() {
    let j = job(Objective::Clean);
    j.stop.store(true, Ordering::Relaxed);
    assert!(j
        .solve(0, true, 4, &Rules::default(), Objective::Clean, false)
        .is_err());
    assert!(j.table.get(code(0, true)).is_none());
}

#[test]
fn incomplete_cache_never_becomes_an_impossibility_certificate() {
    let j = job(Objective::CleanOrDouble);
    let mut p = j.progress.lock().clone();
    for first in [true, false] {
        p.first = first;
        assert!(j.safety_certificate(&p, 100).unwrap().is_none());
    }
}
