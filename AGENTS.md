# Working agreement — wt-benchmarks

The operating contract for **any** coding agent working in this repository. Codex, Cursor
and Gemini CLI read `AGENTS.md` natively; Claude Code loads it through the `@AGENTS.md`
import in [`CLAUDE.md`](CLAUDE.md). Never fork these rules into a per-vendor file.

**Rust** benchmark harness for WorkTable.

## Invariants (do not break these)

- **No Python.** Not a script, not `python3 -c`, not a heredoc. Reaching for it is the
  tell that a step is being solved by parsing when the tool that owns the answer could
  just be asked. Do not swap it for another parser either, and do not assume `jq` is
  present: it does not ship with macOS. A fixed-shape field is one `sed -nE` line;
  anything needing real parsing belongs in this repo's own language, where it can be
  tested. If a task seems to need Python, the approach is wrong.

- **The Python still here is on its way out, not a precedent.** It predates the rule and
  is deleted on sight when the owner finds it, so do not add to it, do not import from it,
  and do not copy its approach into something new. If you are already changing one of
  these, port it out rather than editing it in place:
  - `campaigns/contention/scripts/build-ladder-tables.py` (result tables)
  - `python/kv_json_datatable.py`
