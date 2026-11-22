# The web UI

The web UI is for people at a desk: it shows what is wrong right now, which hosts report
what, and lets operators acknowledge problems. It is part of the server. Open
`https://<server>/` in a browser (or `http://127.0.0.1:8443/` on the machine that runs a
default installation) and log in. There is nothing else to install.

The UI uses the [JSON API](api.md) and nothing else; it makes no request to any other
server, loads no font or script from the internet and stores nothing in the browser except the
session cookie and, on a wall monitor, the token of its display (see
[Wall displays](#wall-displays)).

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
priority. A click on the name opens the problems of that dashboard; "Open on this screen" shows
the dashboard full screen, as a wall monitor would. Below, the wall displays are listed with
what each one is set to show and what it shows now; a click on a display's name opens it. See
[Wall displays](#wall-displays).

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
the address. (The wall monitor keeps the token that is entered on it; see
[Wall displays](#wall-displays).)

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

A wall display is a monitor in an operations room that shows dashboards full screen, without
menus, all day. Nobody logs in on it and nobody needs to touch it. Each display is a file under
`displays/` in the configuration repository: it says which dashboard the display shows, or which
dashboards it rotates through, and whether it switches to a dashboard with an alarm (see
[Display files](configuration-repository.md#display-files-displays)). A display reads its
dashboards with a token of its own, which works for that one display only.

### Setting up a display

Prepare the token at a desk, and do not log in to the UI on the monitor's own browser.

1. Add the display file to the configuration repository, for example `displays/noc.json`, and
   push it. Its id is the file name without `.json`, here `noc`. Once the configuration is in
   sync, the display is listed on the Dashboards page under "Wall displays".
2. At a desk, as an admin, open Configuration → Display tokens and create the display's token.
   It is shown once; copy it.
3. On the monitor's computer, install the start script of [Kiosk mode](#kiosk-mode) below, with
   a browser profile that is kept between starts, but do not register it yet.
4. Start the browser once by hand with the command of the script and the address
   `https://<server>/display/<id>#token`, here `https://<server>/display/noc#token`. `#token` is
   that word, not the token: the token itself never goes into an address. A kiosk window has no
   address bar, so `#token` goes into this one start command.
5. Enter the token under "Display token" and press Start. The page checks the token with the
   server; a wrong one is answered with "This token is not valid for this display." With a valid
   token the display starts, and `#token` is removed from the address.
6. Close the browser (Alt+F4) and register the start script, which opens the address without
   `#token` at every start.

**Do not log in on the monitor.** A display opened in a browser that is logged in to the UI, and
that has no token, runs on that login. Its header then says "Running on your login, not on a
display token", and it stops working when the login ends: after 12 hours without use, and 7 days
after the login at the latest (the defaults of `CERBRIX_AUTH__SESSION_IDLE_SECS` and
`CERBRIX_AUTH__SESSION_MAX_SECS`, see [Login settings](install.md#login-settings)). If you logged
in on the monitor, log out before you open `/display/<id>`. Once a token is entered, the page
ignores any login in the same browser: it sends only the token. That a logged-in user needs no
token is meant for the desk: a click on a display's name on the Dashboards page opens it this way.

**The browser must keep the token.** The token is entered once; the browser keeps it in its
storage for this one page. It is the only thing the UI stores besides the session cookie. A
browser that throws its storage away at every start accepts the token and forgets it at the next
start, without any notice: the monitor then shows the token form again after every restart. That
is the case for a private window (`--incognito`, `--private-window`) and for a temporary profile,
such as a profile directory under `/tmp` or one that is reset at login. Use a normal profile in a
directory that survives a restart. Only when the browser refuses storage outright does the page
say "This browser does not keep the token. It must be entered again after a restart." and start
the display after 10 seconds; such a page also does not reload itself after an upgrade (see
[What the monitor shows](#what-the-monitor-shows)), because the reload would lose the token.

#### Entering or replacing the token

The token form appears when the page has no token, when its token is no longer valid, and when the
address ends in `#token`. To give a monitor a new token, either start its browser once with
`https://<server>/display/<id>#token` (on Linux, stop the service first with
`systemctl --user stop cerbrix-wall.service`, and start it again when the token is entered and
the browser closed), or, where you can reach the address bar, add `#token` to the address of the
running page: the form is shown even though a token is kept, and the new token replaces the old
one. Never put `#token` into the command that starts the browser at every boot: the form would
come back at every start, and the wall would wait for someone to type.

Creating a new token under Configuration (Replace token) makes the old one stop working at once.
Revoking a token, or removing the display from the repository, does the same: at its next request
(they come every 5 seconds) the monitor shows the token form again with "The token of this
display is no longer valid." and forgets the old token.

### What the monitor shows

At the top, the name of the dashboard, the state of the configuration ("In sync", "Out of sync"
or "Not synchronised yet") and the clock; below, the dashboard's panels on its grid of 12 by 6
cells, filling the screen. Nothing scrolls, nothing can be clicked, and the mouse pointer is
hidden after 3 seconds without movement. The display asks the server every 5 seconds, so a new
problem shows within a few seconds.

- **Rotation.** A display set to `rotate` shows each of its dashboards for `every` and then the
  next one, in the order of the file, starting again at the first. A dashboard of the list that
  does not exist is skipped and tried again after 5 minutes; when none of the list can be shown,
  the display tries them again every 5 seconds.
- **Alarm.** A display with `switch_on_alarm` switches to a dashboard that has an alarm: an open,
  unacknowledged problem of at least `min_severity` on one of its hosts. When several dashboards
  have one, the one with the highest priority wins, then the one with the most severe and oldest
  alarm. A band under the header says why. For the display `NOC` of the
  [example](configuration-repository.md#display-files-displays), which rotates through
  `production` and `web`, a critical problem on a production host while "Web servers" is on the
  screen gives "Alarm: showing Production. Switched from Web servers."; when other dashboards have
  an alarm as well, the band adds "1 more dashboard waiting." or "N more dashboards waiting.". A
  second line of the band names the most urgent problem behind the alarm, more severe first, then
  older: "Critical: web-01 – Disk almost full", with "+2 more" at its end when the dashboard has
  further alarms. It is there whether or not the dashboard has a problems panel, so the wall
  always says which host has which problem. The problems that caused the switch are also
  highlighted in the problems panel. When the alarm is
  acknowledged or resolved, the display goes on with the next dashboard with an alarm, or back to
  its own setting; the interrupted dashboard of a rotation gets its full time again.
- **A dashboard that cannot be loaded.** When the dashboard that should be on the screen cannot
  be loaded (above all the dashboard of an alarm), the display does not go on showing the previous
  one: it shows that dashboard's name, the alarm band if there is an alarm, and "This dashboard
  could not be loaded." While the display changes from one dashboard to the next, the previous
  dashboard's panels may stand in, dimmed, under the new name, for at most 5 seconds.
- **Out of sync.** While the last commit of the configuration repository is refused, a band
  says "Out of sync since <time>, showing the configuration from commit <commit>", with the file
  and the reason below it ("and N more" when more files were refused). The display goes on with
  the configuration that is still active.
- **No connection.** When the server has not answered successfully for 15 seconds, the header says
  "No connection to the server since <time>" and the content is dimmed: what is on the screen is
  old. When the server answers, but with an error, it says "The server reported an error since
  <time>" instead. The time is that of the last successful answer. The display keeps trying every
  5 seconds and returns to normal by itself. A restart of the server shows the same way, also
  behind a reverse proxy that answers every request with "404" while the server is away: the
  display does not take that for a display or dashboard that no longer exists, and keeps its
  token.
- **Metrics store down.** When the server cannot reach its metrics store, graph and value panels
  say "Metrics are not available right now"; the problems and host-states panels go on.
- **Upgrades.** The page looks for a new version of the UI on the server every 60 seconds. After
  a server upgrade it reloads itself once, and not again within 10 minutes. A token that the
  browser keeps survives the reload; a page whose browser does not keep the token does not
  reload itself (reload it by hand and enter the token again). If the server goes away exactly
  while the page reloads, the browser shows its own error page and stays there; the start script
  below does not see that, so restart the browser or the computer.

The wall shows no user names and no comments: a problem that somebody acknowledged shows as
"acknowledged", without who or why, because everyone in the room can read the wall. The token
cannot read them either; the server leaves them out of its answers to a display.

**Panels that are too small.** Every panel shows what fits and says what does not. A problems
panel shows as many rows as fit and counts the rest as "and N more". If not even one row fits,
it shows the count with the most urgent severity, for example "Critical · 12 open problems", or,
smaller still, the same in its label ("Critical · 12 open"), so a small panel never hides that
problems are open. Make the panel larger in the dashboard file to see the rows.

**Trying a layout at a desk.** "Open on this screen" on the Dashboards page shows a dashboard
exactly as a wall monitor would, at `/wall/<dashboard>`, for the logged-in user. Moving the mouse
shows the link "Back to the desk" in the header. This is the quickest way to see a change of a
dashboard file before a monitor shows it.

### Kiosk mode

The monitor's computer runs a browser full screen, without its own bars, started with the
computer. Set it up so that it survives a power cut without anybody touching it.

**Wait for the server.** After a power cut the monitor's computer is usually up before the
server. A browser started then shows its own error page ("This site can't be reached") and stays
there: the display's page never loads, so it cannot retry. Start the browser from a small script
that waits until `https://<server>/healthz` answers, and that starts the browser again whenever
it exits. (`/healthz` answers 200 once the server and its database are up.) Once the display's
page is loaded, it rides out a server restart by itself (see "No connection" above).

On Linux, save this as `/usr/local/bin/cerbrix-wall` and make it executable
(`chmod +x /usr/local/bin/cerbrix-wall`); set `SERVER` and `WALL` to your server and display id.
It needs `curl`.

```sh
#!/bin/sh
# Starts the wall display once the Cerbrix server answers, and again whenever the browser exits.
SERVER=https://monitoring.example.net
WALL=noc
PROFILE="$HOME/.local/share/cerbrix-wall"

mkdir -p "$PROFILE"
while true; do
  until curl -fsS -o /dev/null --max-time 10 "$SERVER/healthz"; do
    sleep 5
  done
  chromium --kiosk --app="$SERVER/display/$WALL" \
    --no-first-run --noerrdialogs --hide-crash-restore-bubble \
    --user-data-dir="$PROFILE"
  sleep 5
done
```

Register it as a systemd user service of the user that is logged in automatically on the
monitor's computer (turn on automatic login in the desktop's settings), in
`~/.config/systemd/user/cerbrix-wall.service`:

```ini
[Unit]
Description=Cerbrix wall display
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/local/bin/cerbrix-wall
Restart=always
RestartSec=5

[Install]
WantedBy=graphical-session.target
```

Then, as that user, `systemctl --user daemon-reload` and
`systemctl --user enable --now cerbrix-wall.service`. The `graphical-session.target` is started
by desktops such as GNOME and KDE Plasma; with a desktop that does not start it, put the script
into the desktop's autostart instead (a `.desktop` file in `~/.config/autostart/` with
`Exec=/usr/local/bin/cerbrix-wall`).

On Windows, the equivalent is a scheduled task that runs at logon of the automatically logged-in
user and starts a PowerShell script with the same loop: `Invoke-WebRequest` against `/healthz`
until it succeeds, then `Start-Process -Wait` with the browser and the flags below, and again.

**The browser and its flags.** Chromium (the binary is `chromium-browser` on some
distributions) and Google Chrome (`google-chrome`) take the same flags:

```sh
chromium --kiosk --app=https://<server>/display/<id> --no-first-run --noerrdialogs \
  --hide-crash-restore-bubble --user-data-dir=<persistent directory>
```

| Flag | What it is for |
| --- | --- |
| `--kiosk` | Full screen without the browser's bars. |
| `--app=<address>` | Opens the address as an application window, without tabs. |
| `--no-first-run` | Skips the first-run prompts and pages of a new profile. |
| `--noerrdialogs` | Suppresses the browser's error dialogs. |
| `--hide-crash-restore-bubble` | No "Restore pages?" bubble after the computer lost power: a browser that was not closed counts as crashed. |
| `--user-data-dir=<directory>` | The profile, which keeps the token. Use a directory that survives a restart, not one under `/tmp`. |

Older guides use `--disable-session-crashed-bubble` for the restore bubble; current Chromium no
longer has that flag and ignores it.

For Firefox:

```sh
firefox --kiosk https://<server>/display/<id> --profile <persistent directory>
```

`--kiosk` (Firefox 71 and later) opens the address full screen without the browser's bars;
`--profile` names the profile directory, which keeps the token. Start Firefox once with that
profile in a normal window and close any first-run pages before you use it in kiosk mode.

**Power settings.** Turn off the screen saver, the screen lock, display sleep and system sleep on
the monitor's computer, or the monitor goes dark after a while.

### Screen sizes

The wall is made for Full HD (1920 × 1080) and 4K (3840 × 2160) monitors. Every size, text
included, scales with the width of the window, so a 4K monitor shows the same layout as a Full HD
one, only sharper. Other sizes work as well; the panels always fill the height of the screen.
