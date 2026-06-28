# 0009 — LLM as annotator: the study generation pipeline

**Context.** The headline AI feature is generating teaching studies (commented PGN
with variations). LLMs cannot reliably hold board state — they hallucinate
positions, drop pieces, and suggest illegal lines; a FEN in the prompt does not fix
this. So the model must never reason over a raw position. The one thing it does
better than ChessBase is *language*: explaining ideas, plans, typical structures,
and generating training questions.

**Decision.** The LLM is an **annotator/explainer, never an explorer/calculator.**
All chess truth — legal moves, evaluations, statistics, transpositions — comes from
the engine and the database. The architecture follows from this:

- **Three operation categories, kept distinct:**
  1. *Batch deterministic compute* (tree building, engine calls, DB stats) is
     **orchestration code talking directly to UCI/DB — not an LLM tool.** Engine
     eval/PV must **not** pass through the model context.
  2. *LLM-initiated lookups* (verify a claim, fetch a reference game) — the only
     place runtime tool-calling is appropriate.
  3. *Annotation pass* — the LLM over a finished, tagged tree. Pure language.
- **Two preprocessing layers are STAGES (functions, run without the model), not
  LLM-callable tools:**
  - **Tree builder** — engine+DB → a variation tree pruned by frequency/eval to a
    teachable size. The LLM receives it finished; it never expands it move-by-move.
  - **Feature extractor** — position → concepts the engine does not give (pawn
    structure: IQP/Carlsbad/hedgehog…, open files, **key squares**, king safety,
    material). This is the **center of gravity** of the project: correct key-square
    identification is what creates pedagogical value.
- **One engine/DB service, two facades:** exposed as an **MCP server** for
  interactive use, and called **directly from code** (outside the LLM) for batch.
- **Verification loop:** every concrete LLM claim ("loses a pawn", "only move") is
  checked against engine/DB before being committed. LLM output is a draft.
- **DB tools return pre-chewed data** (ECO, win-rate/frequency, transpositions,
  reference games), never raw games. The model synthesizes, it does not compute.
- **Output artifact:** PGN with NAG glyphs + comments (optionally Lichess study) —
  git-native and versionable.

**Anti-patterns (explicitly avoided).** Letting the LLM expand the tree via a
sequence of tool calls (MCP or internal); sending engine eval/PV into the model
context in batch mode; reducing preprocessing to "send a FEN and ask for a plan";
conflating a *pipeline stage* (function, no model) with an *LLM-callable tool*.

**Consequences.** Studies are correct by construction — the model cannot introduce
illegal lines or invented evals. Most engineering effort goes into the feature
extractor and verification loop, not into prompt-engineering. Two execution modes
share one engine/DB service (Epic 9 batch pipeline vs. interactive MCP). Study
mutation is a programmatic API (issue #18) driven by the human editor and the batch
pipeline — not an LLM runtime tool surface. Tracked as **Epic 9**.
