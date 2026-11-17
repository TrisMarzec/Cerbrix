# The web UI

The web UI is for people at a desk: it shows what is wrong right now, which hosts report
what, and lets operators acknowledge problems. It is part of the server. Open
`https://<server>/` in a browser (or `http://127.0.0.1:8443/` on the machine that runs a
default installation) and log in. There is nothing else to install.

The UI uses the [JSON API](api.md) and nothing else; it makes no request to any other
server, loads no font or script from the internet and stores nothing in the browser except the
session cookie.

## First login

The first user is created on the server with the command line, as described in
[First login](install.md#3-first-login). After that, users are managed in the UI under
Administration. Log in with a user name and a password. After five wrong passwords in a row
the user is locked for five minutes. A session ends after 12 hours without use and after 7
days at the latest (both can be changed, see [Login settings](install.md#login-settings)); the
UI then says that the session has ended and shows the login page. "Change password" and
"Log out" are at the bottom of the navigation.

If you log in as another user in a second tab of the same browser, every other tab switches to
that user as soon as you look at it again: the browser has one session cookie, so all its tabs
are one session.

**Logging out.** If the server cannot be reached when you log out, the page says "Could not log
out: the server did not answer. You are still logged in." The session is then still valid. Try
again, or close the browser; do not leave a shared workstation in that state.

## The pages

The navigation is on the left; on a narrow window it moves to the top. The navigation shows the user and the role, for example "anna · Admin". Every page keeps its
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
closing a problem, a new token) is shown at once, including the count next to "Problems" in the
navigation.

Times and durations follow the server's clock, not the clock of the desk computer, so a wrong
clock on your computer does not make "3 min ago" wrong.

A request that gets no answer within 15 seconds (30 seconds for a change) counts as a lost
connection.

## The bar at the top and the banners

When the configuration comes from Git, the bar at the top of every page shows its state, for
example "In sync · 3f9c2ab · 2 min ago", or "The configuration has not been synchronised yet"
before the first synchronisation has finished. When the configuration is not taken from Git,
the bar says "Configuration is managed on the server" instead. Under the bar there can be
banners:

- **Out of sync**: the last commit of the configuration repository was refused. The banner names
  the file and the reason and says which commit is still active. Monitoring continues with that
  configuration. Fix the file in Git; the next synchronisation clears the banner.
- **No connection to the server since ...**: the server cannot be reached at all. What is on the
  screen is the last data the page got. The banner goes away when the server answers again.
- **The server reported an error since ...**: the server answers, but with an error. The
  banner goes away when it answers normally again.

A page whose own data could not be refreshed, while the rest works, shows "Not updated since
..." above that data.

## Secrets are shown once

An enrolment key, a display token and a generated password are shown in a dialog right after
they are created, with a Copy button. The server stores only a hash of a key or token, so
it cannot show them again: close the dialog only after you have stored the value. The UI
keeps the value only while the dialog is open; it is never written to the browser's storage or
the address.

## Browsers and appearance

The automated check of the UI runs in Chromium only. The current versions of Firefox, Edge and
Safari are expected to work, but nobody tests them automatically. There is one dark theme; it
follows no system setting. Everything can be used with the keyboard, and animations
are off when the system asks for reduced motion.

## Plain HTTP

Serving the UI over plain HTTP under a name other than `localhost` is for testing only. Over
HTTP the browser treats the page as not secure, and that has three visible effects:

- The session cookie has no `Secure` attribute (see `CERBRIX_AUTH__SECURE_COOKIE` in
  [Login settings](install.md#login-settings)), so the browser would also send it over an
  unencrypted connection.
- The browser offers the page no clipboard, so every Copy button asks you to select the text
  and press Ctrl+C instead.
- The browser console shows a message that the `Cross-Origin-Opener-Policy` header is ignored.

`http://localhost` and `http://127.0.0.1` count as secure by the browser and show none of this.
For anything else use HTTPS, for example behind a reverse proxy (see
[the install guide](install.md#behind-a-reverse-proxy)).

## Wall displays

Wall displays, the full-screen dashboards for a monitor on the wall, follow in the next
release. This release lists them, shows what they are set to show and manages their tokens;
the display itself, with the panel grid of a dashboard, is not part of it. A display token is created here now; the wall display page that uses it comes with the next
release.
