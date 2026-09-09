# Rules and conventions

The game is [Gekitai](https://boardgamegeek.com/boardgame/295449/gekitai), designed
by [Scott Brady](https://boardgamegeek.com/boardgamedesigner/117420/scott-brady).
It is the original eight-piece game, not Gekitai² or boop.

## Board and moves

1. Two players start with eight pieces each and an empty 6 × 6 board.
2. On your turn, place one piece on any empty square.
3. Each adjacent piece, including diagonal neighbours and your own pieces,
   moves one square directly away from the newly placed piece if unblocked.
4. A piece cannot push a chain: an occupied destination blocks that push.
5. Pieces pushed off the board return to their owner's supply.
6. Check the win conditions after all pushes have finished.

A player wins by occupying three **consecutive adjacent squares** in a horizontal,
vertical, or diagonal line, or by having all eight pieces on the board. Pieces
with gaps between them do not form a winning line.

The core rules are also described in this [overview by Nicole Brady](https://www.sahmreviews.com/2020/02/gekitai-abstract-strategy-game.html).

## Exact model used by this project

The certificates, default Rust rules, and JavaScript player agree on these
terminal conventions:

| Position after pushing                    | Result                  |
| ----------------------------------------- | ----------------------- |
| Only the mover has a win condition        | Mover wins              |
| Only the other player has a win condition | Mover loses immediately |
| Both players have a win condition         | Draw                    |
| Neither player has a win condition        | Continue                |

The simultaneous-win and repetition conventions are explicit choices of this
implementation. Results are stated under these conventions, rather than as a
claim about every implementation or tournament rule set.

The player ends a repetition as a draw. A repeated state means the same board
up to rotation/reflection with the same player to move. The forcing proof
allows cycles; infinite play does not reach a target. A winning certificate
uses strictly decreasing ranks and therefore cannot cycle. A defensive
certificate proves closure even along cycles.

The CLI can reproduce an earlier orthogonal-push assumption with
`--push-directions orthogonal`, or choose mover precedence with
`--simultaneous-win mover-wins`. Those options do **not** describe the published
strategy. `--repetition forbidden` is only available to the scouting engine;
proof modes reject it because it requires including path history in the state.

## What is verified

A **clean win** means the designated player wins without the opponent also
meeting a win condition. **Clean-or-double** additionally accepts simultaneous
actual lines of three for both players. An all-eight/line combination is not
“simultaneous lines.” See [RESULTS.md](RESULTS.md) for the four independently
checked results and the distinction between a proof and a bounded search.
