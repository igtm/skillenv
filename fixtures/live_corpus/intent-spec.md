---
name: intent-spec
description: >-
  Interactively elicit a user's goals, non-goals, forbidden moves, deferred
  work, constraints, and acceptance criteria, then write a frozen intent
  document to docs/{dir}/{yyyy-mm-dd-name}/intent.md with minimal typing.
---
# Intent Spec

Use this skill when the user has a rough request and wants a durable intent
document before implementation starts.

## Output

Create exactly one canonical file:

`docs/{dir}/{yyyy-mm-dd-name}/intent.md`

Rules for this file:
- It records user intent only.
- It may normalize wording for clarity, but it must not add AI-only
  implementation decisions, architecture choices, estimates, or hidden
  assumptions.
- After it is written, treat it as frozen. Later implementation work must not
  silently edit or "improve" it.
- If the user later changes intent, update it only with explicit user approval
  or create a new versioned document.

## Interaction style

Minimize typing.

- Prefer the dedicated interactive question tool when available.
- Ask in short batches of related multiple-choice questions.
- Always include rich options and keep a free-text escape hatch.
- Put the recommended option first when one is reasonable.
- Ask only for information that is still missing after reading the repository
  and the current conversation.
- If the request is vague, propose 3-5 concrete interpretations and let the
  user pick instead of asking for an open-ended rewrite.
- Reuse prior answers instead of re-asking.
- If the user already answered enough, stop questioning and write the file.

If no dedicated question tool exists, fall back to compact numbered choices in
chat.

## Required capture areas

You must capture all of the following before writing `intent.md`:

1. Requested outcome
2. Why it matters / expected value
3. Primary users or operators
4. In-scope work
5. Out-of-scope work
6. Must-not-do items
7. Not-yet items
8. Constraints and preferences
9. Acceptance criteria
10. Examples, anti-examples, or reference behavior
11. Open choices the user intentionally leaves open

Pay special attention to four boundaries:
- what the user wants
- what the user does not want
- what must never happen
- what should explicitly wait until later

## Directory and naming flow

Determine the destination path first.

1. Ask the user to choose `{dir}` from curated options such as `feature`,
   `fix`, `improvement`, `automation`, `experiment`, `ops`, `docs`, or
   `research`.
2. Derive a short kebab-case `<name>` from the request.
3. Offer 3 slug candidates so the user can choose instead of typing.
4. Use the current date for `yyyy-mm-dd`.

Example:

`docs/feature/2026-03-21-bulk-invite-flow/intent.md`

## Question flow

Use the question sets in `references/question-catalog.md`.

Recommended order:

1. Request shape and destination path
2. Goal and business intent
3. Scope and affected surfaces
4. Non-goals, forbidden moves, and not-yet items
5. Constraints and preferences
6. Acceptance criteria and evidence
7. Examples and edge cases

Do not ask every question blindly. Skip answered sections.

## Writing rules for intent.md

Use the template in `references/intent-template.md`.

Writing rules:
- Keep statements concrete and user-centered.
- Record only what the user said, approved, or clearly implied.
- Mark uncertainty only when the user intentionally leaves something open.
- Do not include an implementation plan.
- Do not include code structure proposals unless the user explicitly asked for
  them.
- Do not include AI-generated TODOs.
- Do not include private reasoning or speculative rationale.

## Completion checklist

Before finishing, verify:
- The path matches `docs/{dir}/{yyyy-mm-dd-name}/intent.md`.
- `intent.md` contains all required capture areas.
- Non-goals, forbidden moves, and not-yet items are explicit.
- The user can hand this file to an implementation agent without extra
  interpretation.
- The file does not contain AI-only implementation decisions.

After writing, tell the user:
- the exact file path
- the chosen title / slug
- the three most important scope guards
