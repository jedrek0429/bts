# Installer transaction boundary

`bts-install` treats installer-owned host state as a transaction for mutating install, add, remove, uninstall and upgrade operations.

Before applying host mutations it records a durable journal under `/var/lib/bts-install/transaction.json`. The journal captures installer state, component configuration files, component activation links, installed BTS systemd units, the CLI activation link, tty1 ownership links and the enabled/active state of BTS services. If a mutation fails, those resources are restored and service state is reconciled to the pre-operation snapshot. If the process is terminated or the machine loses power, the next mutating installer invocation detects the journal and restores the interrupted transaction before planning another operation.

The journal remains pending after component/service reconciliation until the final installer state file has been written successfully. This prevents a failed state write from leaving a deployment that the installer cannot account for.

Downloaded/staged immutable release directories may remain as reusable cache after rollback. Distribution package-manager changes and newly created system accounts are not claimed to be perfectly reversible: removing a package or account automatically could destroy host state that predates or became shared during the transaction. These are explicit transaction boundaries. BTS-owned configuration, activation, unit, tty and recorded service state remain rollback-controlled.

Telephony permission reconciliation only grants the `bts` runtime account membership in the group required to traverse the configured Asterisk sound hierarchy, and the final BTS-generated sound namespace remains owned by `bts`. It does not broaden BTS ownership to unrelated Asterisk state.
