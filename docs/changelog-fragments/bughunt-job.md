Fix: shapes with their own power scale keep it when filled together with other shapes
Fix: the scanning offset now moves image lines the right way when a job is scanned from the top, and leaves one-way scans alone
Fix: a layer without a Z offset no longer runs at the height the layer before it left the head at
Fix: Marlin and LinuxCNC jobs no longer switch the laser on for moves too short to write
Fix: kerf offset now shrinks a hole that is drawn as its own shape inside another shape
Fix: a cropped image now burns only inside its crop
Fix: an image set far too fine to scan is left out with a warning instead of running out of memory
Fix: Cut selected graphics now also holds on a galvo laser
Fix: the material test no longer changes the settings of layers your design already uses
Fix: with an origin set on a G-code machine, the job now runs where its frame runs
Fix: the material test refuses an absurd number of rows or columns instead of crashing
Fix: Start here is forgotten when the design changes, so a changed job no longer starts part way
