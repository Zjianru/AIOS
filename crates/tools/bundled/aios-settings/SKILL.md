---
name: aios-settings
description: Check and change Kind's own setup — the model this conversation uses, masking, connections, usage — with a confirm card the user clicks.
---

# AIOS settings

Use this when the user asks about their own AIOS setup — which model this conversation uses and what it has cost, which models are masked, which connections exist, what AIOS can and cannot do — and when they ask to change one of those.

## When to query

Call aios_query before answering, every time; do not answer these from memory:

- which model and reasoning effort this conversation uses, or what a new conversation would use;
- how much this conversation has cost, how many turns and tokens it took;
- which connections are stored, whether a key is saved, and how the last check went;
- which models each connection can pick, and which are masked;
- what AIOS can and cannot do.

One call per topic. Its output is the only source: quote its numbers, do not recompute or estimate them, and never name a connection, model or price it does not list.

## When to propose

Offer a change only when the user's own words ask for it: switch this conversation's model or effort, mask a model, unmask a model, change settings/connection details, change Keyring handling, or delete the stored Exa key. Match the wish, not keywords.

Offer no card in these cases:

- the user only asks what the setup is now: answer from aios_query and stop;
- the change is already in effect: say so and how to undo it;
- the wish is about a key value: keys never go through Kind or a card; say which settings page field to fill, do not propose. Changing/deleting a connection without its key, changing Keyring handling, or deleting the stored Exa key can be proposed;
- the user has declined before in this conversation, or asks not to be offered anything;
- no registered action matches the wish, or the wish is too vague: answer normally or ask one question.

To propose, call propose_action once, with one option per choice the user can make. The card appears below your answer and nothing changes until the user clicks. Every option carries the everyday name in label and one sentence in description; AIOS fills in the before and after values, so do not write them yourself.

## Actions

- select_model: details is { "model": "<model id>" }, optionally with "connection": "<connection id>" and "effort": "<the model's own effort name>" (leave effort out to keep the current one). Offer one option per model or effort the user could choose.
- mask_model / unmask_model: details is { "connection": "<connection id>", "model": "<model id>" }. Masking blocks that model on every request; unmasking removes the single rule for it. Offer one option, named the way the user names the model.
- settings_update: details carries only the fields the user wants to change: { "seats": 1..16, "bash": true|false, "visionModels": [{"connection":"<id>","model":"<id>"}, ...], "screenCapture": true|false }. visionModels replaces the whole chain; [] clears it. Validate the chain the same way the settings page does (connection exists, model exists, input includes image); facts show the whole chain before → after. screenCapture controls only whether Kind may capture the screen. One card lists just those fields as before → after; existing set_paused handles pause.
- connection_update: only { "id", "endpoint", "model" }; never a key. The card shows address and model before → after.
- connection_delete: { "id" }; connection_revoke: { "id" } (key removed, connection kept). No key value in either.
- keyring_action: { "action": "relocate"|"reopen"|"restart", "path": "<path>" }; relocate/reopen path must start with / or ~/ and appear verbatim in the user's message. If it is not there, ask where; do not propose. restart has no path.
- exa_key_delete: {}; only when the user asks to delete the stored Exa key. The card says web search/web tools stop until the key is filled on the settings page.
- install_package: details is { "id": "pp-ocrv6-small" }. Only this built-in package; it is what vision_ocr uses when the model is not installed. The card lists name/version/size/source/license/location/purpose, and only after the user clicks does AIOS download and check each file's sha256. When vision_ocr returns OCR_MODEL_MISSING, or the user asks for local OCR, load this skill and propose this card once; never download anything yourself.

## One card per turn

A turn shows at most one card. When the user asks for several changes, do the one they asked for first, mention the rest in one sentence, and do not promise a later card. A new card replaces an untouched older one.

## Say why in one sentence

After the call, write your final answer: answer any direct question first, then one sentence on why this pick serves the user's need. Do not restate the card, add steps the card does not state, or claim the change already took effect. The user's click reaches you later as a status notice; if it says the change was refused, say why in one sentence and, when it makes sense, propose again.
