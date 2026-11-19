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

1. Add the display file to the configuration repository, for example `displays/noc.json`, and
   push it. Its id is the file name without `.json`, here `noc`. Once the configuration is in
   sync, the display is listed on the Dashboards page under "Wall displays".
2. As an admin, open Configuration → Display tokens and create the display's token. It is shown
   once; copy it.
3. On the monitor's computer open `https://<server>/display/<id>`, here
   `https://<server>/display/noc`, in a browser (see [Kiosk mode](#kiosk-mode) below).
4. Enter the token under "Display token" and press Start. The page checks the token with the
   server; a wrong one is answered with "This token is not valid for this display." With a valid
   token the display starts.

The token is entered once. The browser keeps it in its storage for this one page; it is the only
thing the UI stores besides the session cookie. Use a normal browser profile, not a private
window, or the token is gone after the browser restarts. If the browser refuses to store it, the
page says "This browser does not keep the token. It must be entered again after a restart." and
starts the display after 10 seconds.

To give a monitor a new token, open `https://<server>/display/<id>#token`: the token form is shown
even though a token is kept, and the new token replaces the old one. Creating a new token under
Configuration (Replace token) makes the old one stop working at once. Revoking a token, or
removing the display from the repository, does the same: at its next request (they come every
5 seconds) the monitor shows the token form again with "The token of this display is no longer valid." and forgets the old token.

Someone who is logged in to the UI in the same browser needs no token: `/display/<id>` then
opens with the session, which is what a click on a display's name on the Dashboards page does.

### What the monitor shows

At the top, the name of the dashboard, the state of the configuration ("In sync", "Out of sync"
or "Not synchronised yet") and the clock; below, the dashboard's panels on its grid of 12 by 6
cells, filling the screen. Nothing scrolls, nothing can be clicked, and the mouse pointer is
hidden after 3 seconds without movement. The display asks the server every 5 seconds, so a new
problem shows within a few seconds.

- **Rotation.** A display set to `rotate` shows each of its dashboards for `every` and then the
  next one, in the order of the file, starting again at the first. A dashboard of the list that
  does not exist is skipped (and tried again after 5 minutes).
- **Alarm.** A display with `switch_on_alarm` switches to a dashboard that has an alarm: an open,
  unacknowledged problem of at least `min_severity` on one of its hosts. When several dashboards
  have one, the one with the highest priority wins, then the one with the most severe and oldest
  alarm. A band under the header says why, for example "Alarm: showing Production. Switched
  from Lobby. 1 more dashboard waiting." The problems that caused the switch are highlighted in
  the problems panel. When the alarm is acknowledged or resolved, the display goes on with the
  next dashboard with an alarm, or back to its own setting; the interrupted dashboard of a
  rotation gets its full time again.
- **Out of sync.** While the last commit of the configuration repository is refused, a band
  says "Out of sync since <time>, showing the configuration from commit <commit>", with the file
  and the reason below it ("and N more" when more files were refused). The display goes on with
  the configuration that is still active.
- **No connection.** When no answer has come from the server for 15 seconds, the header says "No
  connection to the server since <time>" and the content is dimmed: what is on the screen is old.
  When the server answers, but with an error, it says "The server reported an error since
  <time>" instead. The display keeps trying every 5 seconds and returns to normal by itself.
- **Upgrades.** The page looks for a new version of the UI on the server every 60 seconds. After
  a server upgrade it reloads itself once; the token is kept.

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

Run the browser on the monitor's computer full screen, without its own bars, and start it with
the computer. For Chromium or Google Chrome:

```sh
chromium --kiosk --app=https://monitoring.example.net/display/noc
```

For Firefox:

```sh
firefox --kiosk https://monitoring.example.net/display/noc
```

Enter the token once in the kiosk window. Turn off the screen saver, the screen lock and sleep on
that computer, or the monitor goes dark after a while.

### Screen sizes

The wall is made for Full HD (1920 × 1080) and 4K (3840 × 2160) monitors. Every size, text
included, scales with the width of the window, so a 4K monitor shows the same layout as a Full HD
one, only sharper. Other sizes work as well; the panels always fill the height of the screen.
