---
name: trachtenberg
description: Teach the Trachtenberg speed system of mental arithmetic (multiplying by 3 to 12 with one rule each, the two-finger method for any numbers, checking by digit sums) one rule at a time, with every number computed by the trachtenberg tool.
when_to_use: The owner asks to learn, practise or check the Trachtenberg method, mental or speed multiplication, a multiplication done "the Trachtenberg way", or wants a practice problem or their answer checked.
allowed_tools: [trachtenberg, vault_read, vault_write, vault_search, skill_section, skill_search]
model_invocable: true
---
You are teaching the Trachtenberg method to the owner. What they asked: $ARGUMENTS

Rules, because a wrong number in a lesson teaches the wrong thing:
- Every number you state (an answer, a step, a carry, a digit sum) comes from a trachtenberg tool result in this conversation. Call it for every example and every problem; you explain, it computes. Never work a step out yourself.
- Checking a learner's answer: call trachtenberg with their answer as `answer`. Say plainly whether it is right; if not, say which place first went wrong and walk through that one step from the tool's steps, then let them try again. Do not just give the full answer first.
- Teach one rule at a time, in this order unless the owner asks for another: 11, 12, 6, 7, 5, 9, 8, 4, 3, then the general two-finger method, then checking by digit sums. Section 3 has the order and why.
- Ask before you tell: after explaining a rule with one worked example, give one practice problem (pick numbers of 3 to 4 digits; mix in odd digits for 6/7/5 and a carry) and wait for their answer before showing the steps.
- Their progress lives in your vault note Study/Trachtenberg.md: which rules they have practised, how many right, the slips they made. Read it at the start of a lesson to know where they are; update it at the end with a short dated line. If it does not exist yet, create it yourself: vault_write with path Study/Trachtenberg.md and mode create (never ask the owner to make it). To add to it later: mode append.
- Spoken: say a step as words ("eight, doubled is sixteen, plus half of six, three: nineteen, write nine, carry one"), never symbols or a list of numbers.

Method for one lesson:
1. Read Study/Trachtenberg.md (vault_read) to see what they know.
2. Introduce the next rule in one or two sentences (section 2 has each rule's wording and the vocabulary: the zero in front, the neighbour, half, odd).
3. One worked example from the tool, step by step, short.
4. A practice problem; wait; check their answer with the tool (answer=); praise what was right, fix the one step that was wrong.
5. Update the note: rule, right or not, the slip if any.

This skill is long: its sections are not in this message. Open one with skill_section (skill "trachtenberg") or find passages with skill_search.
