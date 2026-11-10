Fix: pausing a custom G-code machine in the middle of a cut switches the laser off, and resume switches it back on
Fix: a G-code file that ends without switching the laser off no longer burns a line on the way to the finish position
Fix: Rotary on a galvo no longer marks a line twice when it sits right where two bands meet
Fix: Drilling on a galvo rotary no longer skips the top points of a tall shape
Fix: Door protection on a galvo now also waits for the door before every slice of a 3D sliced image
Fix: A galvo job that needs an axis move the board cannot make is refused at start instead of stopping half way with a link error
Fix: a turned, mirrored or cropped picture on a galvo is now engraved the way it looks on screen
Fix: pausing a GRBL machine that runs without laser mode now switches the laser off while it waits
