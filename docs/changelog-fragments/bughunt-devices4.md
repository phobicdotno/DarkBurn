Fix: an AI agent can now switch the laser off and stop a jog on a real machine too
Fix: importing preferences or shared defaults never switches AI agent access on
Fix: a GRBL controller with laser mode switched off no longer burns along the travel moves of a job
Fix: a LinuxCNC file runs its end script again (the program used to end before it), and Marlin and Smoothieware files no longer end with a command they do not know
Fix: the G-code an AI agent is shown for a job is now the G-code of the selected laser
Fix: recent projects on a drive that is not plugged in are no longer forgotten
Fix: Check for Updates no longer follows a server that sends it on to an address without https
