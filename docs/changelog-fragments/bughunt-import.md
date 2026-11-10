Fix: a LightBurn file with odd letters inside its path data no longer fails to open
Fix: SVG parts placed with a use element that has its own transform land in the right spot
Fix: PDF outlines that continue after a closed part keep their parts apart
Fix: PDF pages with a user unit come in at their real size
Fix: a damaged PDF layer entry that points at itself no longer closes DarkBurn
Fix: a PDF image whose mask points at itself no longer closes DarkBurn
Fix: a PDF fax image with an absurd width is skipped instead of closing DarkBurn
Fix: Same Width against a line with no width no longer flattens the other shapes
Fix: deleting a layer with its shapes keeps the other shapes of a group
Fix: a shape with a broken coordinate can no longer make a saved project impossible to open
Fix: changes made while a file is still being opened are asked about instead of thrown away
