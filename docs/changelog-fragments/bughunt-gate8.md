Fix: A simulated Gweike run that does not finish in time now says so instead of looking done
Fix: A Gweike plan no longer accepts an acceleration above what the machine allows
Fix: With the Fire button switched off in Device Settings, the fire command no longer fires either
Fix: "Hide Start" for a DSP device now also holds the Start shortcut and menu, not only the button
Fix: A move typed in machine coordinates is refused when it lies outside the machine (Smoothieware)
Fix: With a work origin set, a move to a position is still checked against the machine (Marlin, Smoothieware and custom devices)
Fix: With a work origin set, a frame is still checked against the machine (Marlin, Smoothieware and custom devices)
Fix: Disconnecting or pressing Stop while the Fire button is on now switches the laser off on a Ruida controller, and a job no longer starts with it still on
