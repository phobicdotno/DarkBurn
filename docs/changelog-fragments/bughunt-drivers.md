Fix: a Ruida move or frame with a broken coordinate is refused instead of going to zero
Fix: disconnecting from a GRBL laser that stopped answering no longer freezes the window
Fix: after a stop that the controller did not confirm, commands go through again
Fix: switching air assist no longer turns the fire button off
Fix: a Ruida job now warns when a layer's pauses or cut-through cannot be sent
Fix: Run G-code no longer runs a saved job in the wrong place when the machine has an origin set; it says why instead
Fix: galvo jobs now follow a shape's power scale, cut order priority, kerf offset and start point
Fix: the Rotary and Cylinder correction boxes in the Laser panel now really switch them for the job
Fix: Set finish position works: the job ends there
Fix: saved positions in the Move panel are remembered, one list per device
Fix: jog arrows, Position Laser and move-to-selection follow negative coordinates and swapped axes like the job does
