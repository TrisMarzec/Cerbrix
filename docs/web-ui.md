# The web UI

The web UI is for people at a desk: it shows what is wrong right now, which hosts report
what, and lets operators acknowledge problems. It is part of the server. Open
`https://<server>/` in a browser (or `http://127.0.0.1:8443/` on the machine that runs a
default installation) and log in. There is nothing else to install.

The UI uses the [JSON API](api.md) and nothing else; it makes no request to any other
server, loads no font or script from the internet and stores nothing in the browser.

## First login

The first user is created on the server with the command line, as described in
[First login](install.md#3-first-login). After that, users are managed in the UI under
Administration. Log in with a user name and a password. After five wrong passwords in a row
the user is locked for five minutes. A session ends after 12 hours without use and after 7
days at the latest (both can be changed, see [Login settings](install.md#login-settings)); the
UI then says that the session has ended and shows the login page. "Change password" and
"Log out" are at the bottom of the navigation.

## The pages

The navigation is on the left; on a narrow window it moves to the top. Every page keeps its
filters, tabs, search text and page number in the address, so you can bookmark a view or send
the link to a colleague.

**Dashboards.** One row per dashboard of the configuration repository: its name and file, a
strip of squares with one square for each host (up to 40), the open problems in words, and the
priority. A click on the name opens the problems of that dashboard. Below, the wall displays
are listed with what each one is set to show and what it shows now. A dashboard is not drawn as
a panel grid yet; see [Wall displays](#wall-displays).

**Hosts.** All hosts, with search by name or address and filters for the group and the state
(with problems, no data, without problems). Each row shows the host, its groups, its state,
the current CPU and memory values and when it last reported. A host that has a problem is
marked with the severity word as well as a colour.

**A host.** The page of one host shows its open problems, graphs of its items over the last
hour, 6 hours, 24 hours or 7 days, the current values and the triggers that apply to it. The
graphs show the limits of the triggers as dashed lines. A graph is loaded when it scrolls into
view. If the metrics store does not answer, the graphs say so and the rest of the page keeps
working.

**Problems.** The open problems, or the resolved ones with their end and the reason. You can
search and filter by severity, dashboard and group. Operators and admins see the buttons
Acknowledge (with an optional comment) and Close on every open problem; the row then shows who
acknowledged it and when. Acknowledging does not close the problem; it tells your colleagues that somebody is on it.

**Configuration.** Whether the configuration from Git is in sync: the active commit, the last
successful synchronisation and, if a file was refused, the file, the line and the reason. A
list of the latest synchronisations shows what each one added, changed and removed. Operators
also see the agents that tried to enrol under an unknown host name. Admins can create and
revoke enrolment keys and create, replace and revoke the tokens of wall displays.

**Administration** (admins only). The users: add a user, change a role, disable and enable,
reset a password, remove. The last enabled admin cannot be demoted, disabled or removed.

## What each role sees

| | Viewer | Operator | Admin |
| --- | --- | --- | --- |
| Dashboards, Hosts, host pages, graphs, Problems | yes | yes | yes |
| Configuration: synchronisation state and history | yes | yes | yes |
| Acknowledge and close problems | no | yes | yes |
| Configuration: unknown agents | no | yes | yes |
| Configuration: enrolment keys and display tokens | no | no | yes |
| Release the agent binding of a host | no | no | yes |
| Administration | no | no | yes |

Buttons a role may not use are hidden, but the server decides: every answer "forbidden" is
shown as a message.

## What refreshes by itself

The pages ask the server again every 10 seconds, and the times ("3 min ago") move on at the
same pace. Graphs are updated once a minute. The lists of groups and dashboards used in the
filters are loaded when a page opens. Nothing is requested while the browser tab is hidden;
when you come back, the page refreshes at once. A change you make yourself (an acknowledgement,
a new token) is shown at once.

The bar at the top of every page shows the state of the configuration, for example
"In sync · 3f9c2ab · 2 min ago", and under it two kinds of banner:

- **Out of sync**: the last commit of the configuration repository was refused. The banner names
  the file and the reason and says which commit is still active. Monitoring continues with that
  configuration. Fix the file in Git; the next synchronisation clears the banner.
- **No connection to the server since ...**: the UI could not reach the server (or the server
  answered with an error) at the last attempts. What is on the screen is the last data it got.
  The banner goes away when the server answers again.

## Secrets are shown once

An enrolment key, a display token and a generated password are shown in a dialog right after
they are created, with a Copy button. The server stores only a hash of a key or token, so
it cannot show them again: close the dialog only after you have stored the value. The UI
keeps the value only while the dialog is open; it is never written to the browser's storage or
the address.

## Browsers and appearance

The UI works in the current versions of Firefox, Chrome, Edge and Safari. There is one dark
theme; it follows no system setting. Everything can be used with the keyboard, and animations
are off when the system asks for reduced motion.

## Wall displays

Wall displays, the full-screen dashboards for a monitor on the wall, follow in the next
release. This release lists them, shows what they are set to show and manages their tokens;
the display itself, with the panel grid of a dashboard, is not part of it.
