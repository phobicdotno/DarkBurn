Fix: Boolean, offset and kerf no longer crash on a shape with broken coordinates
Fix: Kerf offset no longer drops a figure-of-eight or a closed line from the job
Fix: Offset and kerf treat an outline that ends on its start point as a closed shape
Fix: Deleting every node of a line inside a bigger shape no longer eats a node of its neighbour
Fix: Breaking or deleting lines at several nodes of a closed shape now hits the nodes you picked
Fix: Convert to curve or line works with every node of an open line selected
Fix: Deleting the last nodes of a shape removes the shape instead of leaving an invisible one
Fix: A mistyped array count or spacing gives a message instead of freezing or closing the program
Fix: Union, Difference, Weld, Cut Shapes and Offset no longer delete a selected picture or line they did not use
Fix: Text no longer vanishes when it is put on a path with no length
Fix: Bent text on a path follows the curve on every edge of a letter
Fix: Bending very large text on a path no longer closes the program
