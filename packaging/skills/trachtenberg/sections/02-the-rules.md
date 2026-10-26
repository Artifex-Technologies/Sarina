# The vocabulary and the rules

Vocabulary: write the number with a zero in front (3452 becomes 03452). Work from the rightmost digit to the left. The neighbour of
a digit is the digit to its right (the rightmost digit has none: its neighbour is 0). "Half" means half rounded down (half of 7 is 3).
"Odd" means the digit itself is 1, 3, 5, 7 or 9. Each step gives a value: write its last digit and carry the rest to the next step.

- ×11: add each digit to its neighbour. 3452 × 11 = 37972.
- ×12: double each digit and add its neighbour. 3452 × 12 = 41424.
- ×6: each digit plus half its neighbour, plus 5 if the digit is odd. 622 × 6 = 3732.
- ×7: double each digit plus half its neighbour, plus 5 if the digit is odd. 2468 × 7 = 17276.
- ×5: half the neighbour, plus 5 if the digit is odd. 426 × 5 = 2130.
- ×9: rightmost digit: 10 minus it; middle digits: 9 minus it, plus the neighbour; the zero in front: the neighbour minus 1.
  2853 × 9 = 25677.
- ×8: rightmost: 10 minus it, doubled; middle: 9 minus it, doubled, plus the neighbour; the zero in front: the neighbour minus 2.
  1234 × 8 = 9872.
- ×4: rightmost: 10 minus it, plus 5 if odd; middle: 9 minus it, plus half the neighbour, plus 5 if odd; the zero in front: half
  the neighbour minus 1. 324 × 4 = 1296.
- ×3: rightmost: 10 minus it, doubled, plus 5 if odd; middle: 9 minus it, doubled, plus half the neighbour, plus 5 if odd; the
  zero in front: half the neighbour minus 2. 571 × 3 = 1713.

These examples were computed by the trachtenberg tool; for a lesson, call the tool for the steps rather than reading them from here.
