@echo off
rem The `feather` command for Command Prompt and PowerShell. The installer
rem puts this folder on the user's PATH.
rem
rem Feather is a windowed program, which a terminal would not wait for, so
rem `feather --help` would print after the next prompt. Run from a script it
rem is waited for; Feather starts its window in the background and returns
rem straight away, so this never holds up the prompt. --from-shell tells it
rem that it is the shell command, even when a script has redirected every
rem stream.
"%~dp0..\feather.exe" --from-shell %*
