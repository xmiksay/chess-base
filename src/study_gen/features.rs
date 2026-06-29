//! Pawn-structure & key-square classification (issue #30): the heuristic layer
//! that turns a raw position into *teachable* strategic concepts the engine does
//! not surface — structure type (IQP, Carlsbad, hedgehog, …), open/half-open
//! files, **key squares** (blockade / bind / outpost squares), a king-safety
//! signal and material imbalance.
//!
//! It is pure and I/O-free (per the architecture layering rule): everything is
//! pattern-matching on the pawn skeleton plus a few board queries from
//! [`crate::position`], so the whole stage is unit-tested against known textbook
//! positions. The variation-tree builder ([`super::tree`]) attaches the output
//! to every node as plain serializable data the LLM later annotates.
//!
//! ## Heuristics (documented, deliberately conservative)
//!
//! Each classifier matches a *signature* on the pawn skeleton — pawns are
//! addressed by `(file, rank)` with both indices `0..=7` (file 0 = a, rank 0 =
//! the 1st rank). A pawn "in front" advances toward rank 7 for White, rank 0 for
//! Black; an enemy pawn attacks the two squares diagonally in front of it. The
//! signatures are intentionally specific so a tag is only emitted when the
//! structure is genuinely present (no eval is consulted — these are facts about
//! the skeleton, not judgements).

use serde::{Deserialize, Serialize};

use crate::position::{
    board_of_fen, pawn_file_counts, pawn_squares, Board, CastlingMode, Color, PositionError,
};

mod derived;

// File indices, for readable signatures.
const A: i32 = 0;
const B: i32 = 1;
const C: i32 = 2;
const D: i32 = 3;
const E: i32 = 4;
const F: i32 = 5;

/// A strategically important square with the side it favours and why. Key
/// squares are the heart of the pedagogical value: the d5 blockade in front of
/// an IQP, the d5 bind in a Maroczy/hedgehog, a knight outpost, a pawn-chain base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySquare {
    /// Algebraic name, e.g. `"d5"`.
    pub square: String,
    /// What kind of key square: `"blockade"`, `"bind"`, `"outpost"`, `"break"`,
    /// `"chain-base"`.
    pub kind: String,
    /// The side that benefits from controlling it: `"white"` or `"black"`.
    pub side: String,
    /// One-line human explanation.
    pub reason: String,
}

/// The strategic concept set for one position. The structured fields carry the
/// machine-usable detail; [`Concepts::tags`] is the flat human-readable summary
/// attached to each variation-tree node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concepts {
    /// Named pawn-structure types, e.g. `"isolated queen's pawn (IQP) for White"`.
    pub structures: Vec<String>,
    /// Key squares with their beneficiary and rationale.
    pub key_squares: Vec<KeySquare>,
    /// Files with no pawns of either colour (file letters).
    pub open_files: Vec<char>,
    /// Files half-open for White (no White pawn, some Black pawn).
    pub white_half_open_files: Vec<char>,
    /// Files half-open for Black (no Black pawn, some White pawn).
    pub black_half_open_files: Vec<char>,
    /// Flat, self-contained concept tags summarising everything above.
    pub tags: Vec<String>,
}

impl Concepts {
    /// Whether no concept at all was detected (used to skip serialization).
    pub fn is_empty(&self) -> bool {
        self.structures.is_empty()
            && self.key_squares.is_empty()
            && self.open_files.is_empty()
            && self.white_half_open_files.is_empty()
            && self.black_half_open_files.is_empty()
            && self.tags.is_empty()
    }
}

/// Classify the pawn structure and key squares of the position described by
/// `fen` (standard chess). Errors only on an invalid / illegal FEN. A
/// backward-compatible convenience over [`concepts_of_fen_with`] for callers
/// that always use standard castling.
pub fn concepts_of_fen(fen: &str) -> Result<Concepts, PositionError> {
    concepts_of_fen_with(fen, CastlingMode::Standard)
}

/// Classify the pawn structure and key squares of `fen` under the given
/// castling `mode` (e.g. [`CastlingMode::Chess960`] for a Fischer-Random start,
/// whose castling rights only parse under that mode). Errors only on an invalid
/// / illegal FEN.
pub fn concepts_of_fen_with(fen: &str, mode: CastlingMode) -> Result<Concepts, PositionError> {
    let board = board_of_fen(fen, mode)?;
    Ok(analyze(&board))
}

/// The pawn skeleton plus per-file counts — the sole input the structure
/// classifiers pattern-match on.
struct Skeleton {
    white: Vec<(i32, i32)>,
    black: Vec<(i32, i32)>,
    wf: [u8; 8],
    bf: [u8; 8],
}

impl Skeleton {
    fn of(board: &Board) -> Self {
        let collect = |color| {
            pawn_squares(board, color)
                .into_iter()
                .map(|sq| (sq.file().to_u32() as i32, sq.rank().to_u32() as i32))
                .collect()
        };
        Self {
            white: collect(Color::White),
            black: collect(Color::Black),
            wf: pawn_file_counts(board, Color::White),
            bf: pawn_file_counts(board, Color::Black),
        }
    }

    fn wp(&self, f: i32, r: i32) -> bool {
        self.white.contains(&(f, r))
    }
    fn bp(&self, f: i32, r: i32) -> bool {
        self.black.contains(&(f, r))
    }
    fn wcount(&self, f: i32) -> u8 {
        if (0..8).contains(&f) {
            self.wf[f as usize]
        } else {
            0
        }
    }
    fn bcount(&self, f: i32) -> u8 {
        if (0..8).contains(&f) {
            self.bf[f as usize]
        } else {
            0
        }
    }
}

/// Algebraic name of a `(file, rank)` square (both `0..=7`).
fn sq_name(file: i32, rank: i32) -> String {
    format!("{}{}", (b'a' + file as u8) as char, rank + 1)
}

fn file_char(file: i32) -> char {
    (b'a' + file as u8) as char
}

/// Run every analyzer and assemble the [`Concepts`].
fn analyze(board: &Board) -> Concepts {
    let sk = Skeleton::of(board);
    let mut c = Concepts::default();

    classify_files(&sk, &mut c);
    classify_structures(&sk, &mut c.structures, &mut c.key_squares);
    derived::add_outposts(&sk, &mut c.key_squares);

    let mut tags: Vec<String> = c.structures.clone();
    tags.extend(file_tags(&c));
    tags.extend(derived::weakness_tags(&sk));
    tags.extend(c.key_squares.iter().map(|k| {
        format!(
            "key square {} ({} for {}): {}",
            k.square, k.kind, k.side, k.reason
        )
    }));
    tags.extend(derived::king_safety_tags(board));
    tags.extend(derived::material_tags(board));
    c.tags = tags;
    c
}

/// Open and half-open files from the per-file pawn counts.
fn classify_files(sk: &Skeleton, c: &mut Concepts) {
    for f in 0..8 {
        match (sk.wf[f], sk.bf[f]) {
            (0, 0) => c.open_files.push(file_char(f as i32)),
            (0, _) => c.white_half_open_files.push(file_char(f as i32)),
            (_, 0) => c.black_half_open_files.push(file_char(f as i32)),
            _ => {}
        }
    }
}

fn file_tags(c: &Concepts) -> Vec<String> {
    let mut tags = Vec::new();
    for &f in &c.open_files {
        tags.push(format!("open {f}-file"));
    }
    for &f in &c.white_half_open_files {
        tags.push(format!("{f}-file half-open for White"));
    }
    for &f in &c.black_half_open_files {
        tags.push(format!("{f}-file half-open for Black"));
    }
    tags
}

/// Named pawn-structure signatures. Order matters only for readability — the
/// signatures are mutually exclusive on the features that distinguish them.
fn classify_structures(sk: &Skeleton, structures: &mut Vec<String>, keys: &mut Vec<KeySquare>) {
    iqp(sk, structures, keys);
    hanging_pawns(sk, structures, keys);
    carlsbad(sk, structures, keys);
    let hedge = hedgehog(sk, structures, keys);
    maroczy(sk, hedge, structures, keys);
    stonewall(sk, structures, keys);
    french_chain(sk, structures, keys);
}

/// Isolated queen's pawn: a lone d-pawn (no friendly c- or e-pawn) with the
/// enemy d-pawn gone. The square directly in front is the blockade square — a
/// permanent outpost for the side playing against the IQP.
fn iqp(sk: &Skeleton, structures: &mut Vec<String>, keys: &mut Vec<KeySquare>) {
    if sk.wcount(D) >= 1 && sk.wcount(C) == 0 && sk.wcount(E) == 0 && sk.bcount(D) == 0 {
        structures.push("isolated queen's pawn (IQP) for White".into());
        if let Some(&(_, r)) = sk.white.iter().find(|(f, _)| *f == D) {
            push_key(keys, D, r + 1, "blockade", "black",
                "blockade square in front of White's isolated d-pawn — an outpost Black cannot be evicted from");
        }
    }
    if sk.bcount(D) >= 1 && sk.bcount(C) == 0 && sk.bcount(E) == 0 && sk.wcount(D) == 0 {
        structures.push("isolated queen's pawn (IQP) for Black".into());
        if let Some(&(_, r)) = sk.black.iter().find(|(f, _)| *f == D) {
            push_key(keys, D, r - 1, "blockade", "white",
                "blockade square in front of Black's isolated d-pawn — an outpost White cannot be evicted from");
        }
    }
}

/// Hanging pawns: a connected c+d (or, mirrored, Black) pawn duo with no
/// friendly b/e neighbours and the enemy c- and d-pawns gone. The squares in
/// front are the blockade points the opponent aims for.
fn hanging_pawns(sk: &Skeleton, structures: &mut Vec<String>, keys: &mut Vec<KeySquare>) {
    hanging_side(&sk.white, &sk.bf, 1, "White", "black", structures, keys);
    hanging_side(&sk.black, &sk.wf, -1, "Black", "white", structures, keys);
}

fn hanging_side(
    mine: &[(i32, i32)],
    enemy_counts: &[u8; 8],
    sign: i32,
    side: &str,
    opp: &str,
    structures: &mut Vec<String>,
    keys: &mut Vec<KeySquare>,
) {
    let has = |f: i32| mine.iter().any(|(pf, _)| *pf == f);
    if has(C)
        && has(D)
        && !has(B)
        && !has(E)
        && enemy_counts[C as usize] == 0
        && enemy_counts[D as usize] == 0
    {
        structures.push(format!("hanging pawns (c & d) for {side}"));
        for f in [C, D] {
            if let Some(&(_, r)) = mine.iter().find(|(pf, _)| *pf == f) {
                push_key(
                    keys,
                    f,
                    r + sign,
                    "blockade",
                    opp,
                    &format!("blockade point in front of {side}'s hanging pawns"),
                );
            }
        }
    }
}

/// Carlsbad (Exchange-QGD) structure: White d4 vs Black d5 with White's c-pawn
/// and Black's e-pawn traded off — the classic minority-attack battleground.
/// White's plan is b2-b4-b5 to fix a weakness on c6.
fn carlsbad(sk: &Skeleton, structures: &mut Vec<String>, keys: &mut Vec<KeySquare>) {
    if sk.wp(D, 3) && sk.bp(D, 4) && sk.wcount(C) == 0 && sk.bcount(E) == 0 && sk.bcount(C) >= 1 {
        structures.push("Carlsbad (minority-attack) structure".into());
        push_key(
            keys,
            B,
            4,
            "break",
            "white",
            "b4-b5 minority-attack break to create a weakness on Black's c-pawn",
        );
        if let Some(&(_, r)) = sk.black.iter().find(|(f, _)| *f == C) {
            push_key(keys, C, r, "outpost", "white",
                "Black's c-pawn — the minority-attack target / backward pawn on the half-open c-file");
        }
    }
}

/// Hedgehog: Black's a6/b6/d6/e6 wall with the c-pawn gone, restrained by White
/// pawns on c4 and e4. White binds d5; Black's freeing breaks are ...b5 and
/// ...d5. Returns whether it matched (so Maroczy doesn't double-tag c4+e4).
fn hedgehog(sk: &Skeleton, structures: &mut Vec<String>, keys: &mut Vec<KeySquare>) -> bool {
    let black_hedge = sk.bp(A, 5)
        && sk.bp(B, 5)
        && sk.bp(D, 5)
        && sk.bp(E, 5)
        && sk.bcount(C) == 0
        && sk.wp(C, 3)
        && sk.wp(E, 3);
    if black_hedge {
        structures.push("hedgehog (Black)".into());
        push_key(
            keys,
            D,
            4,
            "bind",
            "white",
            "d5 — White's bind square; a knight or pawn here cramps the hedgehog",
        );
        push_key(
            keys,
            B,
            4,
            "break",
            "black",
            "...b5 — Black's thematic freeing break",
        );
    }
    black_hedge
}

/// Maroczy bind: White pawns on c4 and e4 clamp d5 with no Black d-pawn to
/// challenge it. Skipped when the fuller hedgehog signature already fired.
fn maroczy(sk: &Skeleton, hedge: bool, structures: &mut Vec<String>, keys: &mut Vec<KeySquare>) {
    if !hedge && sk.wp(C, 3) && sk.wp(E, 3) && sk.bcount(D) == 0 {
        structures.push("Maroczy bind (White)".into());
        push_key(
            keys,
            D,
            4,
            "bind",
            "white",
            "d5 — the Maroczy bind square White clamps with the c4/e4 pawns",
        );
    }
}

/// Stonewall: the c3/d4/e3/f4 (mirrored c6/d5/e6/f5) pawn box that concedes one
/// central square but hands its owner a protected outpost on the 5th (4th) rank.
fn stonewall(sk: &Skeleton, structures: &mut Vec<String>, keys: &mut Vec<KeySquare>) {
    if sk.wp(C, 2) && sk.wp(D, 3) && sk.wp(E, 2) && sk.wp(F, 3) {
        structures.push("Stonewall (White)".into());
        push_key(
            keys,
            E,
            4,
            "outpost",
            "white",
            "e5 — White's Stonewall outpost, a permanent home for a knight",
        );
    }
    if sk.bp(C, 5) && sk.bp(D, 4) && sk.bp(E, 5) && sk.bp(F, 4) {
        structures.push("Stonewall (Black)".into());
        push_key(
            keys,
            E,
            3,
            "outpost",
            "black",
            "e4 — Black's Stonewall outpost, a permanent home for a knight",
        );
    }
}

/// French-type closed centre: locked d4/e5 vs d5/e6 chains. Each side's chain
/// base is the standing target — White's d4 (hit by ...c5), Black's e6 (hit by
/// f4-f5).
fn french_chain(sk: &Skeleton, structures: &mut Vec<String>, keys: &mut Vec<KeySquare>) {
    if sk.wp(D, 3) && sk.wp(E, 4) && sk.bp(D, 4) && sk.bp(E, 5) {
        structures.push("closed centre (French-type pawn chain)".into());
        push_key(
            keys,
            D,
            3,
            "chain-base",
            "black",
            "d4 — base of White's chain, the target of Black's ...c5 break",
        );
        push_key(
            keys,
            E,
            5,
            "chain-base",
            "white",
            "e6 — base of Black's chain, the target of White's f4-f5 break",
        );
    }
}

/// Push a key square unless that square is already recorded (structure-driven
/// entries win, since they carry the more specific reason).
fn push_key(keys: &mut Vec<KeySquare>, f: i32, r: i32, kind: &str, side: &str, reason: &str) {
    if !(0..8).contains(&f) || !(0..8).contains(&r) {
        return;
    }
    let square = sq_name(f, r);
    if keys.iter().any(|k| k.square == square) {
        return;
    }
    keys.push(KeySquare {
        square,
        kind: kind.into(),
        side: side.into(),
        reason: reason.into(),
    });
}

#[cfg(test)]
#[path = "features_tests.rs"]
mod tests;
