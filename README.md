# Cerbrix

Cerbrix is an open-source monitoring system written in Rust that monitors remote hosts in the cloud and on-prem, comparable to Zabbix. Its whole monitoring configuration (hosts, items, triggers, alerting) can live as JSON files in a Git repository, which Cerbrix reconciles periodically at runtime.

With GitOps enabled, the server fetches a Git repository on a schedule, checks it as a whole and applies it only if every file is valid. Otherwise the last valid configuration stays active and the installation shows as Out Of Sync, with the file and the reason. The agents fetch their items and intervals from the server. See [The configuration repository](docs/configuration-repository.md) and [Configuration from Git](docs/install.md#configuration-from-git).

**Status:** early development, not usable yet.

- [Installing the server](docs/install.md)
- [The configuration repository](docs/configuration-repository.md)
- [Design document](docs/superpowers/specs/2026-11-14-cerbrix-design.md)
- [Project board](https://github.com/users/TrisMarzec/projects/2)

## License

Apache-2.0, see [LICENSE](LICENSE).
