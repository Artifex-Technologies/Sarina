//! Go: `go/tiers`: compute tiers. Run on the local backend first, move to the operator's desktop when the device is not
//! enough, and to Claude only as a last resort. The configuration is the operator's (never a workspace's); moving up a
//! tier changes where the model runs, never the tools or permissions; what leaves the machine is sanitised first, and a
//! tier off this computer needs the owner's approval in Heimdall's grants file while that gate is on.

pub mod cli;
pub mod config;
pub mod guard;
pub mod resource;
pub mod router;
pub mod wake;
