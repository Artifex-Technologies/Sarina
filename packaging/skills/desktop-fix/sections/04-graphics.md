# The screen, the graphics card, a black screen or flicker

NVIDIA cards use the nvidia driver (kernel modules built for each kernel). Look with system_status topic hardware (nvidia-smi) and kernel (kernel warnings).

Checks, in order:
1. `nvidia-smi` fails with "couldn't communicate with the NVIDIA driver": the driver modules do not match the running kernel, most often after an update without a restart. `pacman -Q linux-cachyos nvidia-utils` against `uname -r`: if the kernel was updated, the fix is a restart. If it still fails after a restart, the module was not built: the operator runs `sudo mkinitcpio -P` and checks `dkms status` (when the dkms driver is used).
2. Black screen or glitches only in one program: check its own settings first (Vulkan or OpenGL choice, hardware acceleration in browsers).
3. Flicker or tearing in games: variable refresh (VRR/G-Sync) and the game's own V-Sync. Hyprland's `misc:vrr` setting decides VRR; an option for the operator, not something to change silently.
4. A monitor shows the wrong resolution or refresh rate: Settings > Displays, or `hyprctl monitors` to see what Hyprland uses. The best mode is chosen when a monitor is first seen; a different cable or port can limit the refresh rate (HDMI versions).
5. A second monitor is not detected: `hyprctl monitors all` lists disabled ones; the cable, the input chosen on the monitor, and the card's port. Kernel warnings may show link training errors.
6. Temperatures: system_status hardware shows the card's temperature; over about 85 °C under load is worth looking at the fans and dust.

Proof: nvidia-smi answers, and the operator confirms the picture is right.
