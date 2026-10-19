---
description: Bring the zerograph index in line with the tree — a first build where there is no store, else an update
allowed-tools: mcp__plugin_zerograph_zerograph__reindex mcp__plugin_zerograph_zerograph__status
disable-model-invocation: true
---

Call `mcp__plugin_zerograph_zerograph__reindex` with no arguments. It starts `zerograph build` in
the background where the checkout has no store, else `zerograph update`, and returns at once; the
run takes the writer lock as the command line does, so readers keep answering from the store as it
stands. When it says one is already running, nothing new was started.

Then call `mcp__plugin_zerograph_zerograph__status` once and report in one or two lines: what was
started, and how far the run has got. Do not poll it in a loop; the line under the prompt follows
the run, and `/zerograph:status` asks again.

The readers (`ask`, `impact`, `trace`, `changes`, `explain`) already bring the store in line before
they answer, so this is for a first build, or for catching up before a long run of questions.
