# Passwords and their hashes

Stored passwords must be hashed with a slow, salted function made for it: bcrypt ($2b$...), scrypt, Argon2id ($argon2id$...),
sha512crypt ($6$, Linux /etc/shadow), PBKDF2. A plain MD5 or SHA-256 of a password is wrong: fast to guess by the billions per second
and the same for every user who chose the same password (no salt). The salt makes each hash unique; the slowness makes each guess
cost.

What can be done with a password hash: nothing reverses it. A guess can be checked (hash the guess with the same salt and parameters,
compare). Attackers do that with wordlists and GPUs; a long, random passphrase is beyond it, a common word is not. Sarina does not
crack password hashes; she identifies the format (crypto_identify) and says what it means for safety.

Password managers (Proton Pass, Bitwarden) generate and store random passwords and are the right advice; a passphrase of several
random words is right for the one password that unlocks them. Two-factor codes (TOTP, what Proton Authenticator shows) are a shared
secret hashed with the time (HMAC-SHA1 over a 30-second counter); the secret, usually shown once as a QR code, is as sensitive as a
password.

Signs of a password mistake in the owner's own code or files: a password hash that is bare hex (fast hash, no salt), the same hash for
two users, a password or token in a config file or a git history (crypto tools refuse to read .env and key files for that reason).
