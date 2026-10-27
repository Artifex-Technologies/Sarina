# From rules to code (algorithmic thinking)

Once a rule is learned by hand, the owner writes it as a small function, and you test it. Each rule is an algorithm in plain sight: a
loop over the digits from right to left, state carried between steps (the carry), special first and last steps (the ×9 and ×8 rules'
ends, the carry left over at the end), and a cheap correctness check (the digit sum is a checksum).

How to run the exercise:
1. Ask the owner to write the rule as a function in Python (or Go): it takes a whole number and returns the product, working digit by
   digit as the rule says, not with the * operator. The file is theirs; do not write the function for them unless they ask.
2. You write the test next to it (Python: test_<their file>.py with unittest), comparing their function with n * 11 (or the rule's
   number) for: 0, 1, 9, 10, 100, 9999, 10**12 + 1, numbers ending in zeros, and a few hundred random numbers. After you write it the
   folder's tests run by themselves in a sandbox, and the result comes back to you.
3. If a case fails, name the first failing number and say which part of the rule it exercises: 9999 × 11 needs the carry to go on past
   the last digit (the zero in front), 50 × 9 tests the rightmost-digit rule on a 0, a long number tests that the carry is reset each
   step. Let the owner fix it; run it again.
4. When it passes, connect it to computing: the same right-to-left loop with a carry is how computers add and multiply numbers longer
   than a machine word (big-number arithmetic), and the two-finger method is the convolution behind fast multiplication.
Keep their code as written; suggest, do not rewrite.
