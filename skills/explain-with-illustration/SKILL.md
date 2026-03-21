# explain-with-illustration

Use this skill when the user asks for conceptual understanding, architecture explanation, field purpose, or "explain with illustration".

## Output Pattern (must follow in order)

1. **Concept Summary**
- 2-4 lines describing what the component does in plain terms.

2. **Fields and Purpose**
- Explain each important field with:
  - what it stores
  - why it exists
  - what can go wrong without it

3. **Under-the-Hood Flow**
- Step-by-step event flow from input -> state change -> output.
- Keep it deterministic and concrete.

4. **ASCII Illustration**
- Show table/timeline/queue/map transitions.
- Use fixed-width blocks like:

```text
Before:
slot[1]=A, slot[2]=_, inflight=1

Event: ACK(pkid=1)

After:
slot[1]=_, slot[2]=_, inflight=0
```

5. **Assumptions**
- List runtime assumptions explicitly (e.g. pkid != 0, max inflight cap, wraparound bounds).

6. **Edge Cases**
- Include at least 3 likely failure/edge conditions and expected behavior.

7. **Recap**
- 1-2 lines: what to remember and why it matters for the next implementation step.

## Style Rules
- Prefer simple language over protocol jargon.
- Keep examples close to the user's current codebase.
- Do not over-explain unrelated layers.
- If the user asks "why", include tradeoff reasoning.
- If the user asks "how", include API-level sequence and state transitions.

## Repo Context
- Project: `mqtt-client`
- User goal: prepare to maintain `rumqttc`
- Priority: understand invariants, state transitions, and failure handling.

