---
name: aios-settings
description: Check and change Kind's own setup — the model this conversation uses, masking, connections, usage — with a confirm card the user clicks.
---

# AIOS settings

Use this when the user asks about their own AIOS setup — which model this conversation uses and what it has cost, which models are masked, which connections exist, what AIOS can and cannot do — and when they ask to change one of those.

## When to query

Call `aios_query` before answering, every time; do not answer these from memory:

- which model and reasoning effort this conversation uses, or what a new conversation would use;
- how much this conversation has cost, how many turns and tokens it took;
- which connections are stored, whether a key is saved, and how the last check went;
- which models each connection can pick, and which are masked;
- what AIOS can and cannot do.

One call per topic. Its output is the only source: quote its numbers, do not recompute or estimate them, and never name a connection, model or price it does not list.

## When to propose

Offer a change only when the user's own words ask for it: switch this conversation's model or effort, mask a model, unmask a model. Match the wish, not keywords.

Offer no card in these cases:

- the user only asks what the setup is now: answer from `aios_query` and stop;
- the change is already in effect: say so and how to undo it;
- the wish is about a connection or a key: those belong in the settings page; say where, do not propose;
- the user has declined before in this conversation, or asks not to be offered anything;
- no registered action matches the wish, or the wish is too vague: answer normally or ask one question.

To propose, call `propose_action` once, with one option per choice the user can make. The card appears below your answer and nothing changes until the user clicks. Every option carries the everyday name in `label` and one sentence in `description`; AIOS fills in the before and after values, so do not write them yourself.

## Actions

- `select_model`: `details` is `{ "model": "<model id>" }`, optionally with `"connection": "<connection id>"` and `"effort": "<the model's own effort name>"` (leave effort out to keep the current one). Offer one option per model or effort the user could choose.

## One card per turn

A turn shows at most one card. When the user asks for several changes, do the one they asked for first, mention the rest in one sentence, and do not promise a later card. A new card replaces an untouched older one.

## Say why in one sentence

After the call, write your final answer: answer any direct question first, then one sentence on why this pick serves the user's need. Do not restate the card, add steps the card does not state, or claim the change already took effect. The user's click reaches you later as a status notice; if it says the change was refused, say why in one sentence and, when it makes sense, propose again.
