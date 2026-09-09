use super::*;
use crate::board::bit;

fn double_threat() -> Certificate {
    let key = Position {
        us: 0,
        them: bit(0) | bit(1) | bit(28) | bit(29),
    }
    .canonical_key();
    Certificate {
        header: Header {
            rules: Rules::default(),
            objective: Objective::Clean,
            first: false,
            start_key: key,
            forced_depth: Some(2),
        },
        nodes: HashMap::from([(
            code(key, false),
            Node {
                rank: 2,
                chosen: ALL_MOVES,
            },
        )]),
    }
}

#[test]
fn implicit_leaves_and_variable_ranks_preserve_all_reply_obligations() {
    let mut cert = double_threat();
    cert.verify().unwrap();
    // One ply cannot pay for an ongoing reply followed by an implicit win.
    let root = code(cert.header.start_key, false);
    cert.nodes.get_mut(&root).unwrap().rank = 1;
    assert!(cert.verify().is_err());
    cert.nodes.get_mut(&root).unwrap().rank = 5;
    cert.header.forced_depth = Some(5);
    let pos = Position::from_key(cert.header.start_key);
    for mv in generate_moves(&pos) {
        let a = apply_move(&pos, mv, &cert.header.rules);
        if cert.header.objective.terminal(&a, false).is_some() {
            continue;
        }
        let key = a.next.canonical_key();
        let child = Position::from_key(key);
        let win = generate_moves(&child)
            .into_iter()
            .find(|&m| {
                cert.header
                    .objective
                    .terminal(&apply_move(&child, m, &cert.header.rules), true)
                    == Some(true)
            })
            .unwrap();
        cert.nodes.insert(
            code(key, true),
            Node {
                rank: 1,
                chosen: win.0,
            },
        );
    }
    assert!(cert.nodes.len() > 1);
    cert.verify().unwrap();
    // Even a locally winning child cannot have a nondecreasing rank.
    let child = *cert.nodes.keys().find(|&&s| s != root).unwrap();
    cert.nodes.get_mut(&child).unwrap().rank = 5;
    assert!(cert.verify().is_err());
}

#[test]
fn clean_proof_derives_union_and_opponent_avoidance_with_implicit_leaves() {
    let cert = double_threat();
    cert.verify().unwrap();
    cert.derive_from_clean(Objective::CleanOrDouble, false)
        .unwrap()
        .verify()
        .unwrap();
    assert!(cert
        .derive_from_clean(Objective::DoubleOnly, false)
        .is_err());
    for obj in [
        Objective::Clean,
        Objective::CleanOrDouble,
        Objective::DoubleOnly,
    ] {
        let other = cert.derive_from_clean(obj, true).unwrap();
        assert_eq!(other.header.forced_depth, None);
        other.verify().unwrap();
    }
    let mut false_claim = cert
        .derive_from_clean(Objective::CleanOrDouble, true)
        .unwrap();
    false_claim.header.forced_depth = Some(2);
    assert!(false_claim.verify().is_err());
}

#[test]
fn verifier_accepts_terminal_witness_and_rejects_missing_adversarial_replies() {
    let p = Position {
        us: bit(0) | bit(6),
        them: 0,
    };
    let key = p.canonical_key();
    let p = Position::from_key(key);
    let mv = generate_moves(&p)
        .into_iter()
        .find(|&m| {
            Objective::Clean.terminal(&apply_move(&p, m, &Rules::default()), true) == Some(true)
        })
        .unwrap();
    let mut cert = Certificate {
        header: Header {
            rules: Rules::default(),
            objective: Objective::Clean,
            first: true,
            start_key: key,
            forced_depth: Some(1),
        },
        nodes: HashMap::from([(
            code(key, true),
            Node {
                rank: 1,
                chosen: mv.0,
            },
        )]),
    };
    cert.verify().unwrap();
    let dir = std::env::temp_dir().join(format!("gekitai-certificate-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("strategy.certificate");
    cert.save(&path).unwrap();
    Certificate::load(&path).unwrap();
    File::options()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[1])
        .unwrap();
    assert!(Certificate::load(&path).is_err());
    fs::remove_dir_all(&dir).unwrap();
    cert.nodes.insert(
        code(key, true),
        Node {
            rank: 1,
            chosen: ALL_MOVES,
        },
    );
    assert!(cert.verify().is_err());
    // The opponent may end the double-only objective by winning normally.
    cert.header.first = false;
    cert.header.objective = Objective::DoubleOnly;
    cert.header.forced_depth = None;
    cert.nodes = HashMap::from([(
        code(key, false),
        Node {
            rank: 0,
            chosen: mv.0,
        },
    )]);
    cert.verify().unwrap();
    cert.header.first = true;
    cert.nodes = HashMap::from([(
        code(key, true),
        Node {
            rank: 0,
            chosen: mv.0,
        },
    )]);
    assert!(cert.verify().is_err()); // Goal player must have ALL moves covered.
    cert.nodes.insert(
        code(key, true),
        Node {
            rank: 0,
            chosen: ALL_MOVES,
        },
    );
    assert!(cert.verify().is_err()); // Ongoing successors missing.
}
