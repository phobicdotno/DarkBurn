Fix: shapes with their own power scale keep it when filled together with other shapes
Fix: the scanning offset now moves image lines the right way when a job is scanned from the top, and leaves one-way scans alone
Fix: a layer without a Z offset no longer runs at the height the layer before it left the head at
Fix: Marlin and LinuxCNC jobs no longer switch the laser on for moves too short to write
Fix: kerf offset now shrinks a hole that is drawn as its own shape inside another shape
Fix: a cropped image now burns only inside its crop
Fix: an image set far too fine to scan is left out with a warning instead of running out of memory
