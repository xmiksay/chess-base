// Pure FEN helpers — small, framework-free, and unit-tested.

export const STARTPOS_FEN =
  'rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1'

/** Side to move encoded in a FEN: 'white' | 'black'. */
export function sideToMove(fen) {
  const field = String(fen).trim().split(/\s+/)[1]
  return field === 'b' ? 'black' : 'white'
}

/** The piece-placement (first) field of a FEN. */
export function placement(fen) {
  return String(fen).trim().split(/\s+/)[0]
}
