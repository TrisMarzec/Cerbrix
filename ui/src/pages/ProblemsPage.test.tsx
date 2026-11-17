import {
  act,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/preact";

import type { Problem, ProblemsAnswer, Role } from "../api/types";
import { SessionProvider } from "../session";
import { NowContext } from "../shell/context";
import {
  acknowledgedProblem,
  criticalProblem,
  dashboardFixture,
  NOW,
  resolvedProblem,
} from "../test/fixtures";
import { mockFetch, type FetchMock, type Responder } from "../test/mockFetch";
import { advance } from "../test/timers";
import { ProblemsPage } from "./ProblemsPage";

let mock: FetchMock;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  mock.restore();
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
});

function answer(
  items: Problem[],
  extra: { total?: number; dashboards?: ProblemsAnswer["dashboards"] } = {},
) {
  return {
    json: {
      items,
      total: extra.total ?? items.length,
      dashboards: extra.dashboards ?? {},
    },
  };
}

interface Options {
  role?: Role;
  problems: Responder | Responder[];
  routes?: Record<string, Responder | Responder[]>;
}

async function open(url: string, o: Options) {
  window.history.replaceState(null, "", url);
  mock = mockFetch({
    "GET /session": { json: { name: "anna", role: o.role ?? "operator" } },
    "GET /problems": o.problems,
    "GET /dashboards": {
      json: {
        items: [
          dashboardFixture({ id: "e2e", name: "E2E" }),
          dashboardFixture({ id: "prod", name: "Production" }),
        ],
      },
    },
    "GET /host-groups": { json: { items: ["db", "web"] } },
    ...o.routes,
  });
  const view = render(
    <SessionProvider>
      <NowContext.Provider value={NOW}>
        <ProblemsPage />
      </NowContext.Provider>
    </SessionProvider>,
  );
  await advance(0);
  await advance(0);
  await advance(0);
  return view;
}

function problemRequests() {
  return mock.requests.filter(
    (r) => r.method === "GET" && r.path === "/problems",
  );
}

function lastQuery() {
  return problemRequests().at(-1)?.query ?? new URLSearchParams();
}

function region(): HTMLElement {
  const el = document.querySelector<HTMLElement>(".problems-result");
  if (el === null) throw new Error("no result region");
  return el;
}

function rows(): HTMLElement[] {
  return within(screen.getByRole("table", { name: "Problems" }))
    .getAllByRole("row")
    .slice(1);
}

function held(a: ReturnType<typeof answer>) {
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const responder: Responder = async () => {
    await gate;
    return a;
  };
  return { responder, release: () => release() };
}

describe("request", () => {
  it("asks for open problems with a fixed limit by default", async () => {
    await open("/problems", { problems: answer([criticalProblem]) });
    const q = lastQuery();
    expect([...q.keys()].sort()).toEqual(["limit", "offset", "state"]);
    expect(q.get("state")).toBe("open");
    expect(q.get("limit")).toBe("50");
    expect(q.get("offset")).toBe("0");
  });

  it("sends every filter of the URL", async () => {
    await open(
      "/problems?state=resolved&severity=high&dashboard=e2e&group=db&host=db-01&search=disk&offset=50",
      {
        problems: answer([resolvedProblem], { total: 80 }),
      },
    );
    const q = lastQuery();
    expect(q.get("state")).toBe("resolved");
    expect(q.get("severity")).toBe("high");
    expect(q.get("dashboard")).toBe("e2e");
    expect(q.get("group")).toBe("db");
    expect(q.get("host")).toBe("db-01");
    expect(q.get("search")).toBe("disk");
    expect(q.get("offset")).toBe("50");
  });

  it("falls back on garbage in the URL", async () => {
    await open("/problems?state=all&severity=info&offset=-5", {
      problems: answer([criticalProblem]),
    });
    const q = lastQuery();
    expect(q.get("state")).toBe("open");
    expect(q.has("severity")).toBe(false);
    expect(q.get("offset")).toBe("0");
  });

  it("does not send values longer than the API allows and warns", async () => {
    await open(
      `/problems?search=${"a".repeat(201)}&group=${"g".repeat(64)}&host=${"h".repeat(256)}&dashboard=${"d".repeat(201)}`,
      {
        problems: answer([criticalProblem]),
      },
    );
    const q = lastQuery();
    for (const key of ["search", "group", "host", "dashboard"])
      expect(q.has(key)).toBe(false);
    for (const w of ["dashboard", "group", "host", "search"])
      expect(
        screen.getByText(`The ${w} in the address is not valid.`),
      ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Clear filters" })).toHaveAttribute(
      "href",
      "/problems",
    );
  });
});

describe("tabs and filters", () => {
  it("switches to Resolved, changes the URL and resets the offset", async () => {
    await open("/problems?offset=50&severity=high", {
      problems: answer([criticalProblem], { total: 80 }),
    });
    fireEvent.click(screen.getByRole("button", { name: "Resolved" }));
    await advance(0);
    expect(window.location.search).toBe("?severity=high&state=resolved");
    expect(lastQuery().get("state")).toBe("resolved");
    expect(lastQuery().get("offset")).toBe("0");
    expect(screen.getByRole("button", { name: "Resolved" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    await advance(0);
    expect(window.location.search).toBe("?severity=high");
  });

  it("offers the severities and sends the chosen one", async () => {
    await open("/problems", { problems: answer([criticalProblem]) });
    const select = screen.getByRole("combobox", { name: "Severity" });
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual([
      "All severities",
      "Warning and above",
      "High and above",
      "Critical",
    ]);
    fireEvent.change(select, { target: { value: "high" } });
    await advance(0);
    expect(lastQuery().get("severity")).toBe("high");
    fireEvent.change(select, { target: { value: "" } });
    await advance(0);
    expect(lastQuery().has("severity")).toBe(false);
  });

  it("filters by dashboard (names from /dashboards) and by group", async () => {
    await open("/problems", { problems: answer([criticalProblem]) });
    const dash = screen.getByRole("combobox", { name: "Dashboard" });
    expect(
      within(dash)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["All dashboards", "E2E", "Production"]);
    fireEvent.change(dash, { target: { value: "prod" } });
    await advance(0);
    expect(lastQuery().get("dashboard")).toBe("prod");
    fireEvent.change(screen.getByRole("combobox", { name: "Group" }), {
      target: { value: "web" },
    });
    await advance(0);
    expect(lastQuery().get("group")).toBe("web");
    expect(lastQuery().get("dashboard")).toBe("prod");
  });

  it("shows a dashboard that is not in the list as selected, with its id", async () => {
    await open("/problems?dashboard=gone", { problems: answer([]) });
    const dash = screen.getByRole("combobox", { name: "Dashboard" });
    expect(dash).toHaveValue("gone");
    expect(
      within(dash).getByRole("option", { name: "gone" }),
    ).toBeInTheDocument();
  });

  it("searches 300 ms after the last key stroke and resets the offset", async () => {
    await open("/problems?offset=50", {
      problems: answer([criticalProblem], { total: 80 }),
    });
    const input = screen.getByRole("searchbox", { name: "Search" });
    fireEvent.input(input, { target: { value: "d" } });
    await advance(200);
    fireEvent.input(input, { target: { value: "disk" } });
    await advance(299);
    expect(problemRequests()).toHaveLength(1);
    await advance(1);
    expect(window.location.search).toBe("?search=disk");
    expect(lastQuery().get("search")).toBe("disk");
    expect(lastQuery().get("offset")).toBe("0");
  });

  it("does not write the URL for text equal to the URL value, and Clear returns focus", async () => {
    await open("/problems?search=disk", {
      problems: answer([criticalProblem]),
    });
    const input = screen.getByRole("searchbox", { name: "Search" });
    fireEvent.input(input, { target: { value: "disk" } });
    await advance(400);
    expect(problemRequests()).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
    await advance(0);
    expect(window.location.search).toBe("");
    expect(input).toHaveFocus();
    expect(lastQuery().has("search")).toBe(false);
  });

  it("shows the host filter as a chip and removes it", async () => {
    await open("/problems?host=db-02&severity=high", {
      problems: answer([criticalProblem]),
    });
    expect(screen.getByText("Host: db-02")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove host filter" }));
    await advance(0);
    expect(window.location.search).toBe("?severity=high");
    expect(lastQuery().has("host")).toBe(false);
    expect(screen.queryByText("Host: db-02")).not.toBeInTheDocument();
  });
});

describe("rows", () => {
  it("shows host link, problem, severity word, dashboards and times", async () => {
    await open("/problems", {
      problems: answer([criticalProblem], {
        dashboards: {
          "db-01": [
            { id: "e2e", name: "E2E" },
            { id: "a b", name: "Prod" },
          ],
        },
      }),
    });
    const row = rows()[0] as HTMLElement;
    expect(within(row).getByRole("link", { name: "db-01" })).toHaveAttribute(
      "href",
      "/hosts/db-01",
    );
    expect(
      within(row).getByText("Disk almost full mount=/"),
    ).toBeInTheDocument();
    expect(
      within(row).getByText("disk.used_percent > 90: value 94.2"),
    ).toBeInTheDocument();
    expect(within(row).getByText("Critical")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "E2E" })).toHaveAttribute(
      "href",
      "/problems?dashboard=e2e",
    );
    expect(within(row).getByRole("link", { name: "Prod" })).toHaveAttribute(
      "href",
      "/problems?dashboard=a+b",
    );
    expect(within(row).getByText("2 h 14 min")).toBeInTheDocument();
  });

  it("shows a dash for a host in no dashboard and keeps the state in dashboard links", async () => {
    await open("/problems?state=resolved", {
      problems: answer(
        [
          { ...resolvedProblem, host: "a" },
          { ...resolvedProblem, id: "x2", host: "b" },
        ],
        { dashboards: { a: [], b: [{ id: "e2e", name: "E2E" }] } },
      ),
    });
    expect(
      within(rows()[0] as HTMLElement).getByText("–", { selector: "span" }),
    ).toBeInTheDocument();
    expect(
      within(rows()[1] as HTMLElement).getByRole("link", { name: "E2E" }),
    ).toHaveAttribute("href", "/problems?dashboard=e2e&state=resolved");
  });

  it('shows three dashboards and "and N more" with the rest in a title', async () => {
    const list = ["a", "b", "c", "d", "e"].map((id) => ({
      id,
      name: `Dash ${id}`,
    }));
    await open("/problems", {
      problems: answer([criticalProblem], { dashboards: { "db-01": list } }),
    });
    const row = rows()[0] as HTMLElement;
    expect(within(row).getAllByRole("link", { name: /^Dash / })).toHaveLength(
      3,
    );
    expect(within(row).getByText("and 2 more")).toHaveAttribute(
      "title",
      "Dash d, Dash e",
    );
  });

  it("shows who acknowledged and the comment, also on a resolved problem", async () => {
    await open("/problems", { problems: answer([acknowledgedProblem]) });
    const row = rows()[0] as HTMLElement;
    expect(row).toHaveClass("is-acknowledged");
    expect(within(row).getByText(/acknowledged by anna/)).toBeInTheDocument();
    expect(within(row).getByText(/watching/)).toBeInTheDocument();
  });

  it("shows the duration to resolved_at and the reason in words", async () => {
    await open("/problems?state=resolved", {
      problems: answer([
        {
          ...resolvedProblem,
          id: "1",
          host: "h1",
          resolve_reason: "condition_ended",
        },
        {
          ...resolvedProblem,
          id: "2",
          host: "h2",
          resolve_reason: "host_removed",
        },
        {
          ...resolvedProblem,
          id: "3",
          host: "h3",
          resolve_reason: "trigger_removed",
        },
        {
          ...resolvedProblem,
          id: "4",
          host: "h4",
          resolve_reason: "closed_by_operator",
          closed_by: "anna",
          close_comment: "disk removed",
        },
        {
          ...resolvedProblem,
          id: "5",
          host: "h5",
          resolve_reason: "something_new" as never,
        },
      ]),
    });
    const text = rows().map((r) => r.textContent);
    expect(text[0]).toContain("30 min");
    expect(text[0]).toContain("ended by itself");
    expect(text[1]).toContain("host removed");
    expect(text[2]).toContain("trigger removed");
    expect(text[3]).toContain("closed by anna");
    expect(text[3]).toContain("disk removed");
    expect(text[4]).toContain("something_new");
    expect(
      screen.queryByRole("columnheader", { name: "Action" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Acknowledge/ }),
    ).not.toBeInTheDocument();
  });

  it("shows hostile text as text", async () => {
    const evil = "<img src=x onerror=alert(1)>";
    await open("/problems", {
      problems: answer(
        [{ ...criticalProblem, host: evil, trigger: evil, summary: evil }],
        {
          dashboards: { [evil]: [{ id: "x", name: evil }] },
        },
      ),
    });
    expect(document.querySelector("img")).toBeNull();
    expect(
      within(rows()[0] as HTMLElement).getAllByText(
        new RegExp(evil.replace(/[()]/g, "\\$&")),
      ).length,
    ).toBeGreaterThan(2);
  });
});

describe("states", () => {
  it("shows nothing but the heading and controls before the first answer", () => {
    mock = mockFetch({
      "GET /session": { json: { name: "anna", role: "viewer" } },
      "GET /problems": answer([]),
    });
    render(
      <SessionProvider>
        <ProblemsPage />
      </SessionProvider>,
    );
    expect(screen.queryByText("No open problems.")).not.toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("shows both empty sentences", async () => {
    const view = await open("/problems", { problems: answer([]) });
    expect(screen.getByText("No open problems.")).toBeInTheDocument();
    view.unmount();
    mock.restore();
    await open("/problems?state=resolved", { problems: answer([]) });
    expect(screen.getByText("No resolved problems match.")).toBeInTheDocument();
  });

  it('shows "Nothing on this page." for a total without items', async () => {
    await open("/problems?offset=0", { problems: answer([], { total: 3 }) });
    expect(screen.getByText("Nothing on this page.")).toBeInTheDocument();
  });

  it("shows a failed first load with Try again", async () => {
    await open("/problems", {
      problems: [
        { status: 500, json: { error: "internal", message: "x" } },
        answer([criticalProblem]),
      ],
    });
    expect(
      screen.getByText(/Could not load the problems\./),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await advance(0);
    expect(screen.getByRole("table", { name: "Problems" })).toBeInTheDocument();
  });

  it("shows the message of a 400 with Clear filters instead of Try again", async () => {
    await open("/problems?state=resolved&dashboard=gone", {
      problems: {
        status: 400,
        json: { error: "invalid", message: 'unknown dashboard "gone"' },
      },
    });
    expect(screen.getByText(/unknown dashboard "gone"/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Clear filters" })).toHaveAttribute(
      "href",
      "/problems?state=resolved",
    );
    expect(
      screen.queryByRole("button", { name: "Try again" }),
    ).not.toBeInTheDocument();
  });

  it("dims everything of the old answer while the new query is pending, then shows the new one", async () => {
    const slow = held(answer([resolvedProblem]));
    await open("/problems", {
      problems: [answer([criticalProblem]), slow.responder],
    });
    fireEvent.click(screen.getByRole("button", { name: "Resolved" }));
    await advance(0);
    expect(region()).toHaveAttribute("aria-busy", "true");
    expect(region()).toHaveClass("is-busy");
    expect(screen.getByText("1 problem")).toBeInTheDocument();
    slow.release();
    await advance(0);
    expect(region()).toHaveAttribute("aria-busy", "false");
    expect(screen.getByText("Agent silent")).toBeInTheDocument();
  });

  it("shows the failure instead of the old rows when the new query fails", async () => {
    await open("/problems", {
      problems: [
        answer([criticalProblem]),
        { status: 500, json: { error: "internal", message: "x" } },
      ],
    });
    fireEvent.click(screen.getByRole("button", { name: "Resolved" }));
    await advance(0);
    expect(
      screen.getByText(/Could not load the problems\./),
    ).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("hides the empty sentence while a new query is pending", async () => {
    const slow = held(answer([criticalProblem]));
    await open("/problems", { problems: [answer([]), slow.responder] });
    expect(screen.getByText("No open problems.")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Severity" }), {
      target: { value: "high" },
    });
    await advance(0);
    expect(screen.queryByText("No open problems.")).not.toBeInTheDocument();
    slow.release();
    await advance(0);
  });

  it("goes back to the last page when the offset is beyond the total", async () => {
    await open("/problems?offset=100", {
      problems: [
        answer([], { total: 60 }),
        answer([criticalProblem], { total: 60 }),
      ],
    });
    expect(window.location.search).toBe("?offset=50");
    expect(lastQuery().get("offset")).toBe("50");
  });
});

describe("paging", () => {
  it("keeps the filters when paging", async () => {
    await open("/problems?severity=high&host=db-01", {
      problems: answer([criticalProblem], { total: 120 }),
    });
    expect(screen.getByText("1–50 of 120")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await advance(0);
    expect(window.location.search).toContain("offset=50");
    const q = lastQuery();
    expect(q.get("offset")).toBe("50");
    expect(q.get("severity")).toBe("high");
    expect(q.get("host")).toBe("db-01");
  });
});

describe("actions", () => {
  it("gives a viewer no action column", async () => {
    await open("/problems", {
      role: "viewer",
      problems: answer([criticalProblem]),
    });
    expect(
      screen.queryByRole("columnheader", { name: "Action" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Acknowledge/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Close/ }),
    ).not.toBeInTheDocument();
  });

  it("offers Acknowledge again on an acknowledged problem", async () => {
    await open("/problems", { problems: answer([acknowledgedProblem]) });
    expect(
      screen.getByRole("button", { name: /^Acknowledge again/ }),
    ).toBeInTheDocument();
  });

  it("acknowledges through the dialog, reloads at once and shows a notice until the next filter change", async () => {
    await open("/problems", {
      problems: [
        answer([criticalProblem]),
        answer([
          {
            ...criticalProblem,
            acknowledged_at: NOW.toISOString(),
            acknowledged_by: "anna",
          },
        ]),
      ],
      routes: {
        [`POST /problems/${criticalProblem.id}/ack`]: {
          json: { ...criticalProblem },
        },
      },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /^Acknowledge Disk almost full/ }),
    );
    const dialog = screen.getByRole("dialog");
    fireEvent.input(within(dialog).getByLabelText(/Comment/), {
      target: { value: "looking" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Acknowledge" }),
    );
    await advance(0);
    await advance(0);
    const post = mock.requests.find((r) => r.method === "POST");
    expect(post?.body).toEqual({ comment: "looking" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(problemRequests()).toHaveLength(2);
    expect(screen.getByText("Problem acknowledged.")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Severity" }), {
      target: { value: "high" },
    });
    await advance(0);
    expect(screen.queryByText("Problem acknowledged.")).not.toBeInTheDocument();
  });

  it("closes a problem through the dialog and the row goes away", async () => {
    await open("/problems", {
      problems: [answer([criticalProblem]), answer([])],
      routes: {
        [`POST /problems/${criticalProblem.id}/close`]: {
          json: { ...criticalProblem },
        },
      },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /^Close problem Disk almost full/ }),
    );
    const dialog = screen.getByRole("dialog");
    fireEvent.input(within(dialog).getByLabelText("Reason"), {
      target: { value: "removed" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Close problem" }),
    );
    await advance(0);
    await advance(0);
    expect(mock.requests.find((r) => r.method === "POST")?.body).toEqual({
      comment: "removed",
    });
    expect(screen.getByText("Problem closed.")).toBeInTheDocument();
    expect(screen.getByText("No open problems.")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("keeps the dialog open with the message when the server answers 409", async () => {
    await open("/problems", {
      problems: answer([criticalProblem]),
      routes: {
        [`POST /problems/${criticalProblem.id}/ack`]: {
          status: 409,
          json: { error: "conflict", message: "resolved" },
        },
      },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /^Acknowledge Disk almost full/ }),
    );
    const dialog = screen.getByRole("dialog");
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Acknowledge" }),
    );
    await advance(0);
    await advance(0);
    expect(
      within(screen.getByRole("dialog")).getByRole("alert"),
    ).toHaveTextContent("already resolved");
    expect(screen.queryByText("Problem acknowledged.")).not.toBeInTheDocument();
  });
});

describe("polling", () => {
  it("reloads every 10 seconds and keeps the rows when a poll fails", async () => {
    await open("/problems", {
      problems: [answer([criticalProblem]), "network"],
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(problemRequests()).toHaveLength(2);
    expect(screen.getByRole("table", { name: "Problems" })).toBeInTheDocument();
  });
});

describe("rows belong to the query they were fetched for", () => {
  function acknowledgeButtons() {
    return screen.queryAllByRole("button", {
      name: /^Acknowledge|^Close problem/,
    });
  }

  it("has no live action and no Action column for resolved rows after switching to Open", async () => {
    const slow = held(answer([criticalProblem]));
    await open("/problems?state=resolved", {
      problems: [answer([resolvedProblem]), slow.responder],
    });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    await advance(0);
    expect(region()).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("Agent silent")).toBeInTheDocument();
    expect(
      screen.queryByRole("columnheader", { name: "Action" }),
    ).not.toBeInTheDocument();
    expect(acknowledgeButtons()).toHaveLength(0);
    slow.release();
    await advance(0);
    expect(
      screen.getByRole("columnheader", { name: "Action" }),
    ).toBeInTheDocument();
    expect(
      acknowledgeButtons().every((b) => !(b as HTMLButtonElement).disabled),
    ).toBe(true);
    expect(acknowledgeButtons()).toHaveLength(2);
  });

  it("keeps the dashboard links of old rows on the old state while pending", async () => {
    const slow = held(answer([criticalProblem]));
    await open("/problems?state=resolved", {
      problems: [
        answer([resolvedProblem], {
          dashboards: { "web-01": [{ id: "e2e", name: "E2E" }] },
        }),
        slow.responder,
      ],
    });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    await advance(0);
    expect(screen.getByRole("link", { name: "E2E" })).toHaveAttribute(
      "href",
      "/problems?dashboard=e2e&state=resolved",
    );
    slow.release();
    await advance(0);
  });

  it("disables the actions of old rows while a filter change on the Open tab is pending, then they work", async () => {
    const slow = held(answer([criticalProblem]));
    await open("/problems", {
      problems: [answer([criticalProblem]), slow.responder],
    });
    fireEvent.change(screen.getByRole("combobox", { name: "Severity" }), {
      target: { value: "high" },
    });
    await advance(0);
    const buttons = acknowledgeButtons();
    expect(buttons).toHaveLength(2);
    for (const b of buttons) expect(b).toBeDisabled();
    slow.release();
    await advance(0);
    for (const b of acknowledgeButtons()) expect(b).toBeEnabled();
    fireEvent.click(acknowledgeButtons()[0] as HTMLElement);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});

describe("hostile host names and missing data", () => {
  it("shows a dash for hosts named like Object members that are absent from the map", async () => {
    await open("/problems", {
      problems: answer(
        [
          { ...criticalProblem, id: "1", host: "constructor" },
          { ...criticalProblem, id: "2", host: "__proto__" },
        ],
        { dashboards: {} },
      ),
    });
    expect(rows()).toHaveLength(2);
    for (const r of rows())
      expect(
        within(r).getByText("–", { selector: "span" }),
      ).toBeInTheDocument();
  });

  it("reads closed by hand when closed_by is null", async () => {
    await open("/problems?state=resolved", {
      problems: answer([
        {
          ...resolvedProblem,
          resolve_reason: "closed_by_operator",
          closed_by: null,
        },
      ]),
    });
    expect((rows()[0] as HTMLElement).textContent).toContain("closed by hand");
  });

  it("reads acknowledged without by when acknowledged_by is null", async () => {
    await open("/problems", {
      problems: answer([{ ...acknowledgedProblem, acknowledged_by: null }]),
    });
    const text = (rows()[0] as HTMLElement).textContent ?? "";
    expect(text).toContain("acknowledged,");
    expect(text).not.toContain("acknowledged by");
  });

  it("still shows who acknowledged and the comment on the Resolved tab", async () => {
    await open("/problems?state=resolved", {
      problems: answer([
        {
          ...acknowledgedProblem,
          resolved_at: NOW.toISOString(),
          resolve_reason: "condition_ended",
        },
      ]),
    });
    const row = rows()[0] as HTMLElement;
    expect(within(row).getByText(/acknowledged by anna/)).toBeInTheDocument();
    expect(within(row).getByText(/watching/)).toBeInTheDocument();
  });
});
