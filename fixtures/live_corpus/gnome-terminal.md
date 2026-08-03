---
name: gnome-terminal
description: >-
  Open new GNOME Terminal windows from the current working directory in Linux
  development environments. Use when the user asks to launch an extra terminal,
  open a dedicated command terminal (for example for `lumen diff`), or improve
  side-by-side CLI workflows, but only when the current desktop session is GNOME
  and `gnome-terminal` is available.
---
# GNOME Terminal Launcher

Check the environment before launching terminals.

```bash
if [[ "${XDG_CURRENT_DESKTOP:-}" == *"GNOME"* ]] && command -v gnome-terminal >/dev/null 2>&1; then
  echo "ok"
else
  echo "skip"
fi
```

Only run terminal launch commands when the check returns `ok`.

## Open a new terminal at current directory

```bash
gnome-terminal --window --working-directory="$PWD"
```

## Open terminal and run a command

```bash
gnome-terminal --window --working-directory="$PWD" -- bash -lc "lumen diff; exec bash"
```

Use `exec bash` so the new terminal stays open after the command finishes.

## Optional helper script

Use `scripts/open_lumen_diff_terminal.sh` when you want a reusable command wrapper.
