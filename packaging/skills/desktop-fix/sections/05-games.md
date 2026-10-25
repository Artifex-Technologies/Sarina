# A game will not start, crashes, or runs badly

Look first with system_status topic gaming: whether a game runs, the card's use and memory, who holds graphics memory (Sarina's own model too), game mode, Proton versions, installed games.

Checks, in order:
1. Graphics memory full: the gaming topic shows what holds it. Sarina's model (Ollama) holds some while it is loaded; it unloads after a few minutes idle. A game that needs all of the card's memory runs better once it has unloaded.
2. Will not start at all: in Steam, the game's Properties > Compatibility: try another Proton version (the newest Proton, Proton Experimental, or a GE/CachyOS build when installed). Then launch options: `PROTON_LOG=1 %command%` writes `~/steam-<appid>.log`; read its last errors.
3. Starts then crashes: crash_detail for the game's process; the Proton log; for anti-cheat games, check whether the game supports Linux at all (some do not, whatever the setup).
4. Shader compiling every start: Steam's shader pre-caching; the first start after an update compiles shaders (a few minutes is normal), every start is not: check Steam > Settings > Downloads > Shader Pre-Caching and the disk space (a full disk stops the cache).
5. Low frame rate: is game mode on (gaming topic)? Is the game on the NVIDIA card (MangoHud shows it: `MANGOHUD=1 %command%`)? Temperatures (hardware topic). Background programs using the processor (performance topic).
6. Stutter in one place of a game: often shader compiling; it goes after the place was seen once.
7. Controller not seen: Steam Input settings for that game; `ls /dev/input/by-id/` shows connected controllers.

Proof: the game starts and the operator confirms it plays.
