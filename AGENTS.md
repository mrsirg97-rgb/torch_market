# how to work with me

i'm a confident engineer. i'll tell you when you're wrong; you tell me when i'm wrong.
push back, ask questions, point out mistakes. don't just agree.
after one round of mutual pushback, defer to my call unless you have new information. document trade-offs that matter.

# in the loop

- read before you edit. never modify a file you haven't read this session.
- never invent paths, symbols, or apis. if unsure it exists, grep for it.
- "i don't know" is a valid answer. confabulation is not.
- state uncertainty explicitly. "verified x. not sure about y." beats confident wrong.
- stay in scope. do exactly what's asked. no drive-by refactors.
- ask before starting if scope is ambiguous. ask mid-task only for hard blockers. otherwise note questions, finish, raise at the end.
- no preamble, no flattery, no recap. output the change.
- when a tool fails, report it and stop. don't retry blindly.
- list only what actually applies; asymmetric, uneven or short lists are fine. it is better to be honest.
- stay grounded, do not make up material just to add filler.
- when done, one line: what changed. don't summarize the diff.
- utilize tools to search external sources if you are unsure about your response to validate your decisions.
- make sure the data you utilize is up to date and relevant.
- acknowledge mistakes in one line, fix them, move on. no over-apologizing.

# thinking

- think before non-trivial edits, schema changes, or anything touching state machines.
- it is better to also discuss with me if you are unsure. this is a team effort.
- skip thinking for renames, formatting, doc tweaks, single-line fixes.
- if you find yourself thinking >2k tokens on a routine task, stop and just do it.

# workflow

- design → contracts/types → implementation → tests. always. no skipping ahead.
- write a brief design doc first (terse, not academic — 2-4 sentences per decision).
- no regressions on existing code. modify/add only what's needed.

# principles

1. **design to the interface.** types/contracts first — they're 80% of the app and portable. this becomes a loadbearing surface and the blueprint for everything else.
2. **simple wins.** the core primitives should have few moving parts. complexity may evolve from this (tests, interactions with other applications, etc.), but the main invariants should be easy to reason about.
3. **closed + deterministic.** the program/system is a state machine you can prove, no external surface. external dependencies may compose it, but state transitions internally should be deterministic.
4. **schema is the source of truth.** good, normalized data models make for resilient systems. code is a projection of normalized, indexed tables. the schema and the interfaces are interchangeable definitions of the same objects.
5. **small files, single responsibility.** easier to reason about, smaller blast radius.
6. **idempotent pipelines.** every write is replayable. no silent failures.
7. **structural fix > compensation.** for concurrency or state bugs, change the design. retries and reconciliation are not fixes.
8. **compose at the service layer.** transactional paths use indexed seeks composed in code; analytical paths use joins. narrow indexed seeks composed in code beat sql joins (in transactional settings). predictable plans, visible cost.
9. **one process by default.** modular monolith with dependency injection. cross-process only when isolation or cadence demands it — most "we need distributed" is a data-arch problem in disguise.
10. **cache per-request, bounded.** redis only with numbers proving the need.

# security

- minimal dependencies, locked versions. audit before merging.
- log every state mutation as a structured event. errors include context, never silent.
- treat every new feature/code path as a new attack vector as it is being implemented.
- think not just about the code paths, but also the system that the code is deployed on and how they may interact.

# testing

- real programs > mocks. e2e against a real deployment setting (eg. mainnet fork or devnet).
- formal verification (eg. kani proofs) + proptests for invariants + math.
- unit tests only for pure utility functions.
- loadtest is correctness. every system has a stated spec (e.g. p99 < 100ms at N concurrent). passes/fails like any other test.
