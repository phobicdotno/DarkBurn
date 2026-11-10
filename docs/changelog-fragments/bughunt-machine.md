Fix: an AI agent can always stop or pause a real machine
Fix: an AI agent can no longer end the connection to a real machine by switching device
Fix: Stop on a Ruida with nothing running no longer says a job was stopped
Fix: reconnecting right after disconnecting from a laser that stopped answering no longer finds the port busy
Fix: galvo jobs follow the cut direction of a shape and leave its tabs
Fix: the finish position is also written into saved G-code and RD files
Fix: a saved Marlin, Smoothieware, Snapmaker, LinuxCNC or custom G-code file is no longer run shifted on a machine with an origin set
Fix: dot mode with overcut no longer fires the first dots twice
Fix: the frame and the job origin no longer count shapes that are not burned
Fix: fine pictures (300 DPI and up) keep the exact size of their dots in G-code
