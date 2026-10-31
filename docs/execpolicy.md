# Execution policy

For an overview of execution policy rules, see [this documentation](https://developers.openai.com/codex/exec-policy).

When a terminal approval includes a suggested command prefix with arguments,
the menu also offers an executable-only rule, for example `lsof` with any
arguments. The suggested prefix keeps its `p` shortcut; select the broader
choice using the numbered option or arrow keys and Enter. This choice saves
the same kind of persistent allow rule, with only the executable as its prefix.
It is omitted when the suggested prefix already contains only the executable
or names a recognized shell, interpreter, or command wrapper.
