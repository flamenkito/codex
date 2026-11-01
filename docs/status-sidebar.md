# Status sidebar

Wide terminals show a 40-column sidebar alongside the owned transcript. It hides
automatically below 120 columns and while the agents overview occupies the screen.
Inline scrollback mode keeps its existing layout.

The sidebar is always enabled when space permits. There is no toggle command or
shortcut; widening the terminal restores it automatically.

The panel shows session activity and elapsed time, the current goal status and its
time/token usage (including `Pursuing goal`, paused and completed goals), current context usage with comma-separated
token counts, MCP state, workspace, cached branch and dirty-file count,
build identity, and discovered subagents. During a busy model switch it distinguishes
the active turn's model from the next selection. Unknown usage and unavailable metadata are omitted. No cost
estimate is invented for subscription accounts. Opening centered model dialogs
uses the full terminal and dims the sidebar along with the conversation.
The selected model, provider and reasoning effort appear in the prompt status bar.
The prompt text and this metadata row use the same three-column inset from the left gutter.
