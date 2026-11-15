# Cerbrix

Cerbrix is an open-source monitoring system written in Rust that monitors remote hosts in the cloud and on-prem, comparable to Zabbix. Its whole monitoring configuration (hosts, items, triggers, alerting) can live as JSON files in a Git repository, which Cerbrix reconciles periodically at runtime.

With GitOps enabled, the server fetches a Git repository on a schedule, checks it as a whole and applies it only if every file is valid. Otherwise the last valid configuration stays active and the installation shows as Out Of Sync, with the file and the reason. The agents fetch their items and intervals from the server. See [The configuration repository](docs/configuration-repository.md) and [Configuration from Git](docs/install.md#configuration-from-git).

The server evaluates the triggers of the configuration as values arrive. A trigger that fires opens a problem and sends an e-mail to the people named in the notification rules, and a second e-mail when the problem ends; hosts and items that stop reporting are detected as well. Problems are listed and acknowledged with `cerbrix-server admin problem` for now (the web interface comes later). See [Alerting](docs/alerting.md).

**Status:** early development, not usable yet.

- [Installing the server](docs/install.md)
- [The configuration repository](docs/configuration-repository.md)
- [Alerting: problems and e-mail](docs/alerting.md)
- [Design document](docs/superpowers/specs/2026-11-14-cerbrix-design.md)
- [Project board](https://github.com/users/TrisMarzec/projects/2)

## License

Apache-2.0, see [LICENSE](LICENSE).
