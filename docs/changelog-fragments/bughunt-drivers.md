Fix: The fire button now switches off before homing or any other command starts
Fix: Stop during homing now stops the machine right away
Fix: Stop on old GRBL 0.9 controllers no longer ends in an alarm
Fix: Disconnecting in the middle of a job on Marlin and Smoothieware machines now stops the cut right away
Fix: A job started while the fire button was on no longer loses its laser part-way through
Fix: Ruida over network: a late answer from the controller can no longer hide a lost piece of a job
Fix: Stop on a Ruida laser no longer waits behind other commands or an unanswered overwrite question
Fix: Closing the program while it asks whether to overwrite a file on a Ruida controller no longer hangs
Fix: A galvo device set up with a port bit above 15 is now refused with a clear message instead of never finishing its connect
