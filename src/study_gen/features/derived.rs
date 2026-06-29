//! Derived concept tags for the issue #30 feature layer: knight outposts, the
//! isolated/doubled/passed/backward pawn weaknesses, a king-safety signal and
//! material imbalance. These layer on top of the named pawn-structure signatures
//! in [`super`] and share its [`Skeleton`] and key-square helpers.

use super::{file_char, push_key, sq_name, KeySquare, Skeleton, C, F};
use crate::position::{king_square, pawn_file_counts, Board, Color, Role};

/// Add generic knight outposts: defended central squares on the 4th-6th rank
/// (White) / 3rd-5th (Black) that no enemy pawn can ever attack. Deduped against
/// the structure-driven key squares already present.
pub(super) fn add_outposts(sk: &Skeleton, keys: &mut Vec<KeySquare>) {
    for f in C..=F {
        for r in 3..=5 {
            if white_outpost(sk, f, r) {
                push_key(
                    keys,
                    f,
                    r,
                    "outpost",
                    "white",
                    "protected square no Black pawn can attack — an ideal knight post",
                );
            }
        }
        for r in 2..=4 {
            if black_outpost(sk, f, r) {
                push_key(
                    keys,
                    f,
                    r,
                    "outpost",
                    "black",
                    "protected square no White pawn can attack — an ideal knight post",
                );
            }
        }
    }
}

fn white_outpost(sk: &Skeleton, f: i32, r: i32) -> bool {
    let defended = sk.wp(f - 1, r - 1) || sk.wp(f + 1, r - 1);
    let unattackable = !sk
        .black
        .iter()
        .any(|&(bf, br)| (bf == f - 1 || bf == f + 1) && br > r);
    defended && unattackable
}

fn black_outpost(sk: &Skeleton, f: i32, r: i32) -> bool {
    let defended = sk.bp(f - 1, r + 1) || sk.bp(f + 1, r + 1);
    let unattackable = !sk
        .white
        .iter()
        .any(|&(wf, wr)| (wf == f - 1 || wf == f + 1) && wr < r);
    defended && unattackable
}

/// Isolated / doubled / backward / passed pawn tags from the skeleton.
pub(super) fn weakness_tags(sk: &Skeleton) -> Vec<String> {
    let mut tags = Vec::new();
    for (pawns, counts, side, sign, enemy) in [
        (&sk.white, &sk.wf, "White", 1i32, &sk.black),
        (&sk.black, &sk.bf, "Black", -1i32, &sk.white),
    ] {
        let mut isolated = files_of(pawns.iter().filter(|&&(f, _)| {
            counts.get((f - 1) as usize).copied().unwrap_or(0) == 0
                && counts.get((f + 1) as usize).copied().unwrap_or(0) == 0
        }));
        isolated.dedup();
        for f in isolated {
            tags.push(format!(
                "{side} has an isolated pawn on the {}-file",
                file_char(f)
            ));
        }
        for (f, &n) in counts.iter().enumerate() {
            if n >= 2 {
                tags.push(format!(
                    "{side} has doubled pawns on the {}-file",
                    file_char(f as i32)
                ));
            }
        }
        for &(f, r) in pawns {
            if is_passed(f, r, sign, enemy) {
                tags.push(format!("{side} has a passed pawn on {}", sq_name(f, r)));
            }
            if is_backward(f, r, sign, pawns, enemy) {
                tags.push(format!("{side} has a backward pawn on {}", sq_name(f, r)));
            }
        }
    }
    tags
}

fn files_of<'a>(pawns: impl Iterator<Item = &'a (i32, i32)>) -> Vec<i32> {
    let mut files: Vec<i32> = pawns.map(|&(f, _)| f).collect();
    files.sort_unstable();
    files
}

/// A pawn is passed if no enemy pawn stands on its file or an adjacent file
/// anywhere ahead of it (`sign` is +1 for White, -1 for Black).
fn is_passed(f: i32, r: i32, sign: i32, enemy: &[(i32, i32)]) -> bool {
    !enemy
        .iter()
        .any(|&(ef, er)| (ef - f).abs() <= 1 && (er - r) * sign > 0)
}

/// A pawn is backward if it has neighbouring pawns but every one of them is
/// *advanced* beyond it (so none can ever defend its advance) and its stop
/// square is covered by an enemy pawn — the textbook chronic weakness, usually
/// on a half-open file. `sign` is +1 for White, -1 for Black.
fn is_backward(f: i32, r: i32, sign: i32, own: &[(i32, i32)], enemy: &[(i32, i32)]) -> bool {
    let neighbours: Vec<i32> = own
        .iter()
        .filter(|&&(pf, _)| (pf - f).abs() == 1)
        .map(|&(_, pr)| pr)
        .collect();
    if neighbours.is_empty() {
        return false; // isolated, not backward
    }
    // No neighbour is level with or behind this pawn — none can ever support it.
    let unsupported = neighbours.iter().all(|&pr| (pr - r) * sign > 0);
    // The stop square (one ahead) is attacked by an enemy pawn (two ahead, on an
    // adjacent file).
    let stop_attacked = enemy
        .iter()
        .any(|&(ef, er)| (ef - f).abs() == 1 && er == r + 2 * sign);
    unsupported && stop_attacked
}

/// King-safety signal per side: castled wing, intact pawn shield, exposure.
pub(super) fn king_safety_tags(board: &Board) -> Vec<String> {
    let mut tags = Vec::new();
    for color in [Color::White, Color::Black] {
        let Some(king) = king_square(board, color) else {
            continue;
        };
        let kf = king.file().to_u32() as i32;
        let kr = king.rank().to_u32() as i32;
        let side = if color == Color::White {
            "White"
        } else {
            "Black"
        };
        let home = if color == Color::White { 0 } else { 7 };
        let own = pawn_file_counts(board, color);
        let enemy = pawn_file_counts(board, color.other());

        let wing = if kf <= C {
            "queenside"
        } else if kf >= F {
            "kingside"
        } else {
            "centre"
        };

        // Shield = friendly pawns on the king's file and its neighbours that are
        // still ahead of the king (toward the enemy).
        let shield = (kf - 1..=kf + 1)
            .filter(|&f| (0..8).contains(&f) && own[f as usize] > 0)
            .count();
        let open_near: Vec<char> = (kf - 1..=kf + 1)
            .filter(|&f| (0..8).contains(&f) && own[f as usize] == 0 && enemy[f as usize] > 0)
            .map(file_char)
            .collect();

        let castled = kr == home && wing != "centre";
        let mut note = format!("{side} king on the {wing}");
        if castled && shield >= 3 && open_near.is_empty() {
            note.push_str(", pawn shield intact");
        } else if !open_near.is_empty() {
            let files: String = open_near
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("/");
            note.push_str(&format!(", exposed on the {files}-file(s)"));
        } else if shield < 3 {
            note.push_str(", pawn cover loosened");
        }
        tags.push(note);
    }
    tags
}

/// Material-imbalance signals beyond the raw point count: the bishop pair and
/// opposite-coloured bishops (both decisive for the right plan).
pub(super) fn material_tags(board: &Board) -> Vec<String> {
    let mut tags = Vec::new();
    let wb = board.material_side(Color::White).bishop;
    let bb = board.material_side(Color::Black).bishop;
    if wb >= 2 && bb < 2 {
        tags.push("White has the bishop pair".into());
    }
    if bb >= 2 && wb < 2 {
        tags.push("Black has the bishop pair".into());
    }
    if wb == 1 && bb == 1 && opposite_coloured_bishops(board) {
        tags.push("opposite-coloured bishops".into());
    }
    tags
}

/// Whether each side's single bishop sits on opposite-coloured squares.
fn opposite_coloured_bishops(board: &Board) -> bool {
    let colour_of = |color: Color| {
        let bishops = board.by_piece(Role::Bishop.of(color));
        bishops.into_iter().next().map(|sq| sq.is_light())
    };
    match (colour_of(Color::White), colour_of(Color::Black)) {
        (Some(w), Some(b)) => w != b,
        _ => false,
    }
}
