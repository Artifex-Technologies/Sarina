# Symmetric and asymmetric encryption

Symmetric: AES (128/256-bit keys) and ChaCha20 are the workhorses; both sides hold the same key. A cipher needs a mode: ECB encrypts
each 16-byte block alone (identical blocks give identical ciphertext: the "ECB penguin" still shows the picture), so it is wrong for
almost everything; CBC chains blocks with an IV; CTR and GCM turn the cipher into a stream, and GCM (or ChaCha20-Poly1305) also
authenticates, so tampering is detected. Use an authenticated mode; use a fresh random IV or nonce every time (a reused nonce with
GCM or a stream cipher leaks the XOR of the two plaintexts).

Asymmetric: a key pair. RSA (2048 bits or more today; 1024 is broken-adjacent) encrypts small things and signs; elliptic curves
(P-256, Curve25519/Ed25519) do the same with shorter keys and faster. In practice asymmetric crypto moves or agrees a symmetric key
(TLS, PGP, SSH) and symmetric crypto carries the data; this is hybrid encryption. Key exchange (Diffie-Hellman, X25519) lets two
parties agree a secret over a public channel; with fresh keys per session that is forward secrecy: a stolen long-term key later does
not open old traffic.

Keys and passwords differ: a key is random bits of the right size; a password is low-entropy and must go through a key derivation
function (see Hashes, MACs, signatures and KDFs) before it can be a key.

What a tool here can do: say what kind of key or certificate something is (crypto_modern), recognise ciphertext by its entropy
(crypto_identify), and nothing more: decrypting without the key is not possible, and the honest answer says so.
