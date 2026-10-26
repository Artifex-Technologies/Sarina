# Checking by digit sums

Add a number's digits, and again until one digit is left; 9 counts as 0 (so 324 → 9 → 0). Multiply the two factors' digit sums and
reduce that the same way; it must equal the answer's digit sum. 3452 × 11: 5 × 2 = 10 → 1, and 37972 → 28 → 10 → 1: it agrees.
If it disagrees, the answer is certainly wrong. If it agrees, the answer is very likely right, but the check misses some slips
(two digits swapped, a 0 written for a 9). The trachtenberg tool reports the check with every result, and says which place first
went wrong when a learner's answer is checked.
