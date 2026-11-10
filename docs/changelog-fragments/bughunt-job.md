Fix: Kerf offset on a cut sub-layer no longer grows the fill of the same layer
Fix: Each cut sub-layer now uses its own kerf offset
Fix: Scanning offsets now also apply to fills
Fix: The time estimate now counts the fast whitespace scan
Fix: Images burn the way Adjust Image shows them in Sketch mode and with noise reduction
Fix: Cropped-away and transparent parts of an image no longer burn when the image is negative or darkened, and crop edges stay sharp
Fix: Perforation mode now leaves a bridge on shapes shorter than the cut length too
Fix: Reduce travel moves now also shortens the path between shapes that are not grouped
Fix: The order of Layer, Group and Priority in Order by now counts: group first finishes each group before the next
Fix: A picture is now placed and framed by its own rectangle, white edges included, instead of by its first and last dot
Fix: Offset fill no longer burns across holes, and fills a shape inside a hole only once
Fix: Offset fill now fills letters and other shapes with arms or narrow necks completely, with rings around holes
