Fix: On a galvo, Stop pressed right after Start now always wins
Fix: A galvo job reads the door switch again at the moment it starts
Fix: Resume on a galvo reads the door switch again first
Fix: A galvo job that waits for the foot switch no longer starts on an old press
Fix: Disarming a galvo while a job is marking now stops the job
Fix: Go to origin on Smoothieware, Marlin and custom G-code machines no longer runs to an origin outside the machine
Fix: On Marlin, a jog, move or frame after Set origin no longer runs past the end of the machine
Fix: On the Gweike, a jog key held down no longer keeps the head moving once the card asks for a restart
Fix: On a Ruida controller, a jog, move, home or frame now ends the Fire button first
Fix: After a lost connection the app no longer shows the laser as firing
Fix: A job or frame started while the head still jogs is refused, instead of losing its first lines
Fix: DSP controllers: Fire, moves and a second Start are refused right after Start too, not only once the job shows as running
Fix: Ruida: Fire and Start are refused while a frame is on its way or running
Fix: A device with the Fire button switched off refuses to fire in the driver too
