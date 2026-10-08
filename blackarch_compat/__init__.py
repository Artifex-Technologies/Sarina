"""BlackArch category compatibility test harness.

Installs BlackArch tool categories one at a time (or in explicit
combinations) inside disposable Docker containers and records whether
each install succeeds cleanly, building up a compatibility matrix before
any specific set of categories is baked into a distro image's default
package list.

This is a build/test tool for the future distro, not a feature of the
Sarina agent itself — same relationship this repo's ``benchmarks/``
package has to the agent.
"""
