# Cerbrix

Cerbrix is an open-source monitoring system written in Rust that monitors remote hosts in the cloud and on-prem, comparable to Zabbix. Its whole monitoring configuration (hosts, items, triggers, alerting) can live as JSON files in a Git repository, which Cerbrix reconciles periodically at runtime.

With GitOps enabled, the server fetches a Git repository on a schedule, checks it as a whole and applies it only if every file is valid. Otherwise the last valid configuration stays active and the installation shows as Out Of Sync, with the file and the reason. The agents fetch their items and intervals from the server. See [The configuration repository](docs/configuration-repository.md) and [Configuration from Git](docs/install.md#configuration-from-git).

The server evaluates the triggers of the configuration as values arrive. A trigger that fires opens a problem and sends an e-mail to the people named in the notification rules, and a second e-mail when the problem ends; hosts and items that stop reporting are detected as well. Problems are listed and acknowledged in the web UI or with `cerbrix-server admin problem`. See [Alerting](docs/alerting.md).

The server has a web UI for people at a desk, served at `/`: dashboards, hosts with graphs, problems with acknowledgement, the state of the configuration and the administration of users (wall displays follow in the next release). See [The web UI](docs/web-ui.md). The UI uses the JSON API under `/api/v1`. Users log in with a name and a password and have one of three roles: viewer (read), operator (also acknowledge and close problems) or admin (also manage users, enrolment keys and display tokens). Wall displays read their dashboards with a token of their own. The first user is created with `cerbrix-server admin user create`; see [the API reference](docs/api.md) and [Login settings](docs/install.md#login-settings).

**Status:** early development, not usable yet.

- [Installing the server](docs/install.md)
- [The configuration repository](docs/configuration-repository.md)
- [Alerting: problems and e-mail](docs/alerting.md)
- [The JSON API](docs/api.md)
- [Design document](docs/superpowers/specs/2026-11-14-cerbrix-design.md)
- [Project board](https://github.com/users/TrisMarzec/projects/2)

## License

Apache-2.0, see [LICENSE](LICENSE).
