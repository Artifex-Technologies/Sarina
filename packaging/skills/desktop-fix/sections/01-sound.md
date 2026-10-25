# No sound, or sound from the wrong device

The sound server is PipeWire with WirePlumber (user services, no sudo). Look with system_status topic audio (it shows `wpctl status`).

Checks, in order:
1. Is there an output at all? In `wpctl status`, under Audio > Sinks: no sinks means PipeWire lost the card. Fix: system_fix restart_audio, then look again.
2. Only when every program is silent: is the right output the default? The default sink has a `*` in front. Wrong one (HDMI on the graphics card instead of speakers or headset): the operator picks it in the top bar's sound menu or Settings > Sound, or runs `wpctl set-default <id>` with the sink's number.
3. Is it muted or at 0? `wpctl get-volume @DEFAULT_AUDIO_SINK@` says `[MUTED]` or a volume. Fix: `wpctl set-mute @DEFAULT_AUDIO_SINK@ 0` and `wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.5`.
4. Only one program is silent while others play (then skip steps 2 and 3: the default output works, the other programs prove it): the problem is that program's own stream, not the default output (do not change the default for this). In `wpctl status` under Audio > Streams, find the program's stream and its number. Muted or at 0: `wpctl set-mute <stream number> 0` and `wpctl set-volume <stream number> 1.0`. Playing to another output: move it in pavucontrol (Playback tab, the output next to the program). No stream at all while it should play: the program itself is not sending sound (its own volume or settings, a tab that is muted in a browser, or restart the program).
5. Bluetooth headphones sound bad or the microphone does not work: the headset profile (HSP/HFP, mono, phone quality) and the music profile (A2DP) cannot both be used; the microphone needs the headset profile. Explain the trade-off; the profile is chosen in the sound settings.
6. HDMI or DisplayPort sound on an NVIDIA card is missing: the monitor must be on and on that input before the sink appears; after turning it on, restart_audio.
7. Still nothing: `systemctl --user status pipewire wireplumber pipewire-pulse` (system_status services shows failed user services). A failed one: restart_audio. Crashing again and again: crash_detail for it.

Proof it works: the sink is listed, is the default, unmuted, and the operator hears a test sound (`pw-play /usr/share/sounds/freedesktop/stereo/bell.oga` when that file exists).
