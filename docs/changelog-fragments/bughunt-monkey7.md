Fix: Convert to Bitmap no longer puts the picture on a layer that does not exist
Fix: An ellipse or rounded rectangle with an absurd size no longer eats all the memory
Fix: A picture placed absurdly far off the page no longer crashes the job
Fix: A command that changes nothing no longer leaves an empty undo step or marks the project as changed
Fix: Shapes blown up to kilometres on a fill layer no longer crash the program, they are left out of the job with a warning
Fix: Cut Shapes on a few hundred shapes takes a moment instead of most of a minute
Fix: A shape can no longer be moved or sized out of this world (past a thousand kilometres), which put "infinity" into the job
Fix: A QR code or barcode of an absurd size is refused instead of crashing
Fix: An ellipse with a size near the largest number there is no longer takes all the memory
Fix: Adding a tab to a shape that has no size is refused instead of quietly breaking undo
Fix: Two-Point Rotate / Scale onto one and the same target point no longer shrinks the shape to a dot
