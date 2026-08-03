---
name: spec-implementation-loop
description: >-
  Implement work from a frozen intent document at
  docs/{dir}/{yyyy-mm-dd-name}/intent.md, run test and real-data validation
  loops, wait autonomously with sleep when needed, track tech notes, score
  quality, and continue improving until the quality bar is met.
---
# Spec Implementation Loop

Use this skill after an intent file already exists and the user wants the work
implemented with minimal interruption.

## Required input and sibling outputs

Input:
- `docs/{dir}/{yyyy-mm-dd-name}/intent.md`

Required outputs in the same directory:
- `docs/{dir}/{yyyy-mm-dd-name}/tech-notes.md`
- `docs/{dir}/{yyyy-mm-dd-name}/implementation-report.md`

Never edit `intent.md` during implementation. Treat it as frozen user intent.
If reality conflicts with it, record the conflict in `tech-notes.md`, choose the
closest safe path, and ask the user only if a hard blocker remains.

## Working style

Keep moving.

- Do as much as possible without asking the user to babysit the process.
- Prefer autonomous loops over status pings.
- Use the repository's normal build, test, lint, and verification commands.
- Use real data or the smallest safe production-like dataset available.
- Record important findings continuously in `tech-notes.md`, not only at the
  end.
- When a long-running step needs time, wait and poll instead of stopping.

## Wait behavior

When waiting for servers, migrations, queues, browsers, CI, indexing, or remote
jobs:

- use polling loops
- sleep between checks
- re-check health or status
- continue automatically once ready

Examples:

```bash
until curl -fsS http://localhost:3000/health >/dev/null; do sleep 2; done
while ! python scripts/check_job.py; do sleep 5; done
```

Prefer short, safe sleep intervals over repeated user interruptions.

## Execution loop

### 0. Load the contract

- Read `intent.md` fully.
- Extract goals, scope guards, constraints, must-not-do items, and acceptance
  criteria.
- Build a short private checklist for yourself.
- Do not rewrite the contract.

### 1. Start documentation immediately

Create or update `tech-notes.md` right away using
`references/tech-notes-template.md`.

Track at least:
- assumptions being tested
- repo and environment findings
- experiment results
- failures and fixes
- real-data observations
- remaining risks

### 2. Implement the thinnest viable slice

- Start with the smallest end-to-end change that satisfies the highest-value
  acceptance criteria.
- Stay inside the scope guards from `intent.md`.
- Align with repository conventions and existing architecture.

### 3. Verify locally

- Run fast checks first.
- Then run the full relevant test, build, lint, and smoke flow.
- Fix failures before moving on.

### 4. Validate with real data

- Run the feature against real accessible data, real endpoints, real files, or
  the closest safe live-like dataset.
- Choose the smallest representative real dataset first.
- Capture input shape, environment, outcome, and surprises in `tech-notes.md`.
- If safe real-data validation is blocked, exhaust accessible alternatives,
  document the gap, and ask one targeted question only after all non-blocked
  work is complete.

### 5. Score quality

Use the rubric in `references/quality-rubric.md`.

Default pass bar:
- total score >= 88 / 100
- intent fidelity >= 22 / 25
- automated verification >= 16 / 20
- real-data validation >= 16 / 20

### 6. Improve and loop

If the pass bar is not met:

- identify the highest-impact score gaps
- update `tech-notes.md`
- implement the next improvement
- re-run the affected checks and real-data validation
- score again

Repeat until the pass bar is met or a hard external blocker remains.

## Hard blocker rules

A blocker is hard only when at least one of the following is true:

- a required secret or credential is missing
- the user intent is internally contradictory
- a destructive or policy-sensitive action needs explicit approval
- a required external system stays unavailable after reasonable retries

Before asking the user, finish every non-blocked task and summarize:
- what was completed
- what was tried
- what evidence was collected
- the single exact thing now needed

## Required result document

Write `implementation-report.md` at the end using
`references/report-template.md`.

It must include tables for:
- requirement coverage
- verification runs
- real-data experiments
- quality score by category and by loop
- alternative implementation approaches with comparative scores
- why the chosen approach won

If there are screenshots, videos, logs, benchmark files, or other artifacts,
link their paths in the report.

## Final completion checklist

Before finishing, verify:
- `intent.md` was not edited during implementation
- `tech-notes.md` captures meaningful discoveries from implementation and
  real-data work
- `implementation-report.md` explains the chosen approach and compares it
  against alternatives
- the final quality score and evidence are explicit
- the user can understand what shipped, how it was validated, and what tradeoffs
  were made
