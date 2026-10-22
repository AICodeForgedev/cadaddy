# A weak agent on the synthetic hotel (Q320)

Do the agent tools (`bim_job` over MCP, Q319) let an agent with a small context and no memory
finish a building? An evaluation runs the synthetic hotel (a made-up project, `cadaddy-mepfix`)
through MCP only: import the prints, build, clear the review queue, take off quantities and list
the RFIs — forgetting everything every few steps and resuming from `bim_job briefing`.

`crates/mcp/tests/weakagent.rs`:

- **Scripted agent** (in the normal test suite; no model, no network). It holds only the last
  answers within its context budget; every *n* steps (or when the budget is spent) it forgets
  everything and resumes from the briefing. For a run whose size or height is a guess it "reads the
  documents" (the hotel's truth, as a person reads the tag on the sheet) and resolves it citing the
  sheet; about one tag in eight is unreadable and must be deferred. Clashes become RFIs. A refused
  answer is never retried: the item goes to a person with the reason.
- **Real model** (on demand only, never in CI): `WEAKAGENT_MODEL=<model id> ANTHROPIC_API_KEY=…
  cargo test -p cadaddy-mcp --test weakagent real_model_run -- --ignored --nocapture`. The model
  gets only the `bim_job` tool; its conversation is cut every 8 tool calls; every call is logged
  (`WEAKAGENT_LOG`, JSON lines). No keys are kept in the repository.

The project is written as an import leaves it (levels, the hotel's duct and pipe runs, every other
one at its system's default size and height). The import step itself (`bim_job import`) opens and
classifies the 46 printed sheets; the print readers that turn sheets into the building spec run in
`cadaddy-mepfix`, outside the engine, so the evaluation provides their result.

## Scores (scripted agent, 10-21)

Accuracy: share of the project's runs whose size and bottom match the documents. Tokens per item:
characters the agent read, / 4, per item decided.

| context (chars) | forgets every | steps | tool calls | resolved | deferred | RFIs | wrong edits | tool errors | resumes | lost track | builds | accuracy before → after | tokens/item |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 8,000 | 6 steps | 44 | 100 | 14 | 3 | 23 | 0 | 0 | 10 | 0 | 2 | 0.48 → 0.88 | 501 |
| 4,000 | 3 steps | 44 | 115 | 14 | 3 | 23 | 0 | 0 | 25 | 0 | 2 | 0.48 → 0.88 | 733 |
| 2,000 | 2 steps | 43 | 131 | 14 | 3 | 23 | 0 | 0 | 43 | 0 | 2 | 0.48 → 0.88 | 986 |

The runs left wrong (0.88) are the three whose tags were unreadable: deferred to a person, as they
should be. The test asserts every column that matters: finished, no wrong edits, no tool errors, no
lost track, a rebuild after the answers, accuracy up by more than 0.3, under 1,500 tokens per item.

## What the weak agent tripped on (each fixed, each with a test)

1. **Its own RFIs came back as new work.** An RFI drafted from a clash went onto the list *open*, so
   the next `review_next` handed it back to the agent. Drafted RFIs are now *sent* (waiting for the
   design team) and stay out of the open queue.
2. **The same question, asked 1,984 times.** Two clashes with the same title: drafting the second's
   RFI found the first's and answered "unchanged" without settling the second item, which stayed
   first in the queue forever. The second item is now settled by the existing RFI.
3. **Takeoff from a stale model.** After the answers changed the spec, the briefing pointed straight
   to the takeoff. It now says to build again first when the spec changed since the last build.
4. **No way to get the RFI list at the job level.** `bim_job rfis` returns the 20 most severe
   (compact) or exports the whole list to a file.
5. **`bim_job import` answered with nothing**, and an unknown action came back as a protocol error
   rather than a tool error: the tool returned past the server's result wrapping. Fixed.

Engine regression tests: `crates/engine/tests/agentready.rs` (`what_the_weak_agent_tripped_on`);
MCP: the weak agent itself and the suite entry for `bim_job`.
