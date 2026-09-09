use gekitai_solver::{
    movegen::{apply_move, generate_moves, MoveOutcome},
    position::Position,
    rules::Rules,
};
#[test]
#[ignore = "writes reference cases for the JavaScript port"]
fn export_browser_rules_reference() {
    let mut rng = 87531u64;
    let mut cases = Vec::new();
    let rules = Rules::default();
    let mut pos = Position::start();
    let mut first_turn = true;
    for _ in 0..10000 {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let moves = generate_moves(&pos);
        let mv = moves[rng as usize % moves.len()];
        let applied = apply_move(&pos, mv, &rules);
        let board = |p: Position, first: bool| -> Vec<u8> {
            let (x, o) = if first {
                (p.us, p.them)
            } else {
                (p.them, p.us)
            };
            (0..36)
                .map(|i| {
                    if x & (1 << i) != 0 {
                        1
                    } else if o & (1 << i) != 0 {
                        2
                    } else {
                        0
                    }
                })
                .collect()
        };
        let winner = match applied.outcome {
            MoveOutcome::Ongoing => None,
            MoveOutcome::Draw => Some(0),
            MoveOutcome::MoverWin => Some(if first_turn { 1 } else { 2 }),
            MoveOutcome::MoverLoss => Some(if first_turn { 2 } else { 1 }),
        };
        cases.push(serde_json::json!({"board":board(pos,first_turn),"turn":if first_turn {1} else {2},"square":mv.0,"next":board(applied.next,!first_turn),"winner":winner,"key":pos.canonical_key().to_string()}));
        if applied.outcome == MoveOutcome::Ongoing {
            pos = applied.next;
            first_turn = !first_turn;
        } else {
            pos = Position::start();
            first_turn = true;
        }
    }
    std::fs::create_dir_all("test/evidence").unwrap();
    std::fs::write(
        "test/evidence/static-rules.json",
        serde_json::to_vec(&cases).unwrap(),
    )
    .unwrap();
}
