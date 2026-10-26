# What cannot be known, and how to say it

- A hash cannot be reversed. Possible: compare with the hash of a guess; look it up in a list of hashes of common strings (not something
  Sarina does; she says it exists).
- Encrypted data (AES, ChaCha20, RSA, a PGP message, an encrypted zip) cannot be read without its key or password. No tool here does it;
  no tool anywhere does for a correct implementation. Possible: find the key where it was stored, ask the owner of the key.
- A signature is verified only with the right key. A decoded JWT, a certificate's text, a PGP packet list: facts the data claims about
  itself. crypto_modern writes VERIFIED only when it checked with a key given to it.
- A certificate's trust: "valid now" is about dates. Whether a browser would trust it depends on the chain to a root, which the tool
  does not check.
- A password hash: only guesses can be tested; a strong password is not findable.
- Who made a file, when, and whether a string is "malicious": a string is bytes; what it does depends on who runs it where.
- Whether something is random: statistics can say it looks random (entropy near 8); they cannot say it is encrypted rather than
  compressed, or which cipher.

Say the limit in one sentence and the realistic next step in another. "I cannot" with the reason is a complete and correct answer;
inventing a plaintext, a key or a verdict is the one thing that must never happen.
