"""Chymaera ecosystem integration helpers.

Sarina is one of four repos in the Chymaera ecosystem (see ``docs/ECOSYSTEM.md``).
This package holds the code that talks to the sibling projects:

- ``firmware`` — read-only advisories over a firmware checkout. Reports only.
- ``console`` — client for the Chymaera_Zero_App control API. May record
  sessions and log entries in that app's datastore.

Nothing here flashes, transmits, or reconfigures hardware. Those stay human
actions by agreement with the firmware project; see ``docs/ECOSYSTEM.md``.
"""

from .console import (
    DEFAULT_PORT as CONSOLE_DEFAULT_PORT,
    ConsoleClient,
    ConsoleError,
    ConsoleUnavailable,
    render_status,
)
from .firmware import (
    DeviceInfoReport,
    MergeImpactReport,
    check_device_info,
    merge_impact,
    parse_device_info,
)

__all__ = [
    'CONSOLE_DEFAULT_PORT',
    'ConsoleClient',
    'ConsoleError',
    'ConsoleUnavailable',
    'DeviceInfoReport',
    'MergeImpactReport',
    'check_device_info',
    'merge_impact',
    'parse_device_info',
    'render_status',
]
