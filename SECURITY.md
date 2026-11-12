# Security

## Which versions get fixes

DarkBurn is pre-release and no release is published yet. Fixes go into the newest code on `main` only.

## How to report

Report privately. On the repository page open the **Security** tab and press **Report a vulnerability**.

If the button is not there, open an [issue](https://github.com/phobicdotno/DarkBurn/issues) that asks for a private channel. Give no detail in the issue.

Please say what you did, what happened, which system you were on and which commit you built. A file that shows the problem helps most.

## What counts

- A project, design, picture or settings file that makes DarkBurn run code, or read or write outside the place you chose.
- The AI agent channel (`darkburn-mcp`) doing something its switches forbid: reaching a machine with machine control off, starting a job or firing without the confirmation, changing its own switches, or answering to something that is not on this computer. The safety model is in [crates/darkburn-mcp/README.md](crates/darkburn-mcp/README.md).
- An install script (`scripts/install.sh`, `scripts/install.ps1`) doing something it should not: running a download that was cut off, writing or deleting outside its own folders, or fetching from somewhere else than it says.
- The update check following an address it should not follow.

## Safety reports

Anything that could make a laser fire or move when it should not is wanted too, and with the same urgency. That is safety rather than security, but report it the same way. Examples: a laser that stays on after Stop, Pause or a dropped connection, a job that starts without being asked for, a move outside the work area.

DarkBurn has not yet burned a job on a real machine. The machine drivers have run on the built-in simulators and loopback links only, so reports from real hardware are the ones most likely to find such faults.
