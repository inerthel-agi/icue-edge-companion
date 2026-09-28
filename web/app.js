// Companion window (design v2): sidebar, five pages, status footer.
(function () {
  "use strict";
  const U = window.Usage, esc = U.esc;
  const content = document.getElementById("content");
  let page = "overview";
  let info = null;
  let lastHtml = "";
  let helpOpen = false;
  let sp = null;
  let spClientId = "";

  const ICON_ALERT = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/></svg>';
  const pill = (p) => '<span class="pill ' + (p.state === "idle" ? "" : p.state) + '">' + esc(p.stateText) + "</span>" + (p.refreshing ? ' <span class="pill busy">Updating</span>' : "");
  const bar = (u, extra) => '<div class="bar ' + U.level(u) + (extra ? " " + extra : "") + '"><i data-w="' + u + '"></i></div>';
  // 24 h chart and pace estimate for the headline limit (Claude only).
  const TREND = { claude: true };
  const alert = (text, cls) => '<div class="alert ' + (cls || "") + '">' + ICON_ALERT + "<span>" + esc(text) + "</span></div>";

  // ---------- overview ----------
  function providerCard(p) {
    const t = U.tightest(p);
    let hero;
    if (!p.detected) hero = alert(p.reason || p.name + " not detected", "warn");
    else if (!t) hero = alert(p.limitsError || "No limits reported.", p.state === "danger" ? "" : "warn");
    else {
      const eta = TREND[p.id] ? U.forecast(t) : null;
      hero = '<div><div class="hero-figure"><b>' + t.pct + '%</b><span>of ' + esc(t.name.toLowerCase()) + " " + t.word + "</span></div>" +
        U.limitBar(t, t.stale ? "off" : "") + '<p class="hero-meta">Resets ' + U.resetText(t.resetAt) + (t.stale && t.reason ? " · " + esc(t.reason) : "") + "</p>" +
        (eta ? '<p class="hero-meta' + (eta.warn ? " eta-warn" : "") + '">' + esc(eta.text) + "</p>" : "") + "</div>";
    }
    const others = t ? p.limits.filter((l) => l !== t).map((l) =>
      '<div class="mini-row"><span>' + esc(l.name) + "</span><b>" + l.pct + "%</b>" + U.limitBar(l, l.stale ? "off" : "") + "</div>").join("") : "";
    const reason = t && p.state === "danger" && p.reason ? alert(p.reason) : "";
    const ctx = p.context ? U.big(p.context.used) + (p.context.capacity ? ' <small class="muted">/ ' + U.big(p.context.capacity) + "</small>" : "") : "—";
    return '<section class="card">' +
      '<div class="card-head"><span class="mark ' + esc(p.id) + '">' + p.letter + "</span><div><strong>" + esc(p.name) + "</strong><small>" + esc(p.clients) + "</small></div>" + pill(p) + "</div>" +
      reason + hero + (others ? '<div class="mini">' + others + "</div>" : "") +
      '<div class="facts"><div class="fact"><span>Tokens today</span><b>' + U.big(p.tokens.total) + "</b></div>" +
      '<div class="fact"><span>Current conversation</span><b>' + ctx + "</b></div></div>" +
      '<div class="card-foot"><span>Limits read ' + U.ago(p.limitsAt) + '</span><button type="button" class="link" data-go="' + p.id + '">View details →</button></div>' +
      "</section>";
  }

  // ---------- overview: AI usage, media and screens ----------
  const NOTE = '<svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M9 18.5a3 3 0 1 1-2-2.83V5.2l12-2.2v12.5a3 3 0 1 1-2-2.83V6.4l-8 1.47z"/></svg>';
  const fmtTime = (sec) => { sec = Math.max(0, Math.floor(sec)); return Math.floor(sec / 60) + ":" + String(sec % 60).padStart(2, "0"); };

  function nowPlayingCard(m) {
    const s = m && m.session;
    const players = m ? m.sessions.length : 0;
    const state = !s ? ["Idle", ""] : s.playback === "playing" ? ["Playing", "live"] : s.playback === "paused" ? ["Paused", ""] : ["Unknown", ""];
    let body;
    if (!m) body = '<p class="muted">Media sessions are not available.</p>';
    else if (!s) body = '<p class="muted">Nothing playing. Start playback in any app shown in the Windows media flyout.</p>';
    else {
      const t = s.timeline;
      let prog = "";
      if (t && t.duration) {
        const pos = Math.min(t.duration, t.position + (s.playback === "playing" ? Math.max(0, Date.now() - t.updatedAt) / 1000 : 0));
        prog = '<div class="np-prog">' + bar(Math.round(pos / t.duration * 100)) + '<div class="np-times"><span>' + fmtTime(pos) + "</span><span>" + fmtTime(t.duration) + "</span></div></div>";
      }
      body = '<div class="np"><div class="np-art">' + (s.art ? '<img src="' + esc(s.art.url) + '" alt="">' : NOTE) + "</div>" +
        '<div class="np-text"><b>' + esc(s.title || "Untitled") + "</b><span>" + esc(s.artist || "") + "</span><small>" + esc(s.app.name) + "</small></div></div>" + prog;
    }
    return '<div class="card"><div class="card-head"><span class="mark media">' + NOTE + "</span><div><strong>Now Playing</strong><small>Windows media · " +
      players + (players === 1 ? " player" : " players") + '</small></div><span class="pill ' + state[1] + '">' + state[0] + "</span></div>" + body +
      '<div class="card-foot"><span>Shown on Now Playing and Windows Media Pump</span></div></div>';
  }

  function spotifyCard(s) {
    const [label, cls] = s ? SP_STATUS[s.status] || SP_STATUS.error : ["Loading", "busy"];
    let body;
    if (!s || s.status === "not_configured") body = '<p class="muted">Connect your own Spotify app to control playback and show synced lyrics on the XENEON EDGE.</p>';
    else if (s.item) body = '<div class="np-text"><b>' + esc(s.item.title) + "</b><span>" + esc(s.item.artists) + "</span><small>" +
      (s.device ? esc(s.device.name) + (s.device.volume == null ? "" : " · volume " + s.device.volume + "%") : "No active device") + " · lyrics " + esc(s.lyrics) + "</small></div>";
    else body = '<p class="muted">' + (s.device ? "Nothing playing on " + esc(s.device.name) + "." : "No active device. Start Spotify on a computer, phone or speaker.") + "</p>";
    return '<div class="card"><div class="card-head"><span class="mark spotify">' + U.LOGO.spotify + '</span><div><strong>Spotify</strong><small>' +
      (s && s.account ? esc(s.account.name) : "Your own Spotify app") + '</small></div><span class="pill ' + cls + '">' + label + "</span></div>" + body +
      '<div class="card-foot"><span>Shown on Spotify</span><button type="button" class="link" data-go="spotify">' + (s && s.status === "not_configured" ? "Connect" : "View details") + " →</button></div></div>";
  }

  function screensGroup() {
    const i = info || {};
    const n = (i.widgets || []).filter((w) => w.live && w.screen === "XENEON EDGE").length;
    const pump = i.pumpRelay || {};
    const row = (title, sub, right) => '<div class="row"><div class="row-text"><span>' + title + "</span><small>" + sub + "</small></div>" + (right || "") + "</div>";
    return '<div class="group">' +
      row("XENEON EDGE", n ? n + (n === 1 ? " widget connected" : " widgets connected") + " right now" : "No widget connected. Add them in iCUE.",
        '<span class="pill ' + (n ? "live" : "") + '">' + (n ? n + " connected" : "None") + "</span>") +
      row("Pump LCD", pump.installed ? "Windows Media Pump · artwork and progress written " + (pump.writtenAt ? U.ago(pump.writtenAt) : "soon") : "Windows Media Pump is not installed",
        '<span class="pill ' + (pump.installed ? "live" : "") + '">' + (pump.installed ? "Relay on" : "Off") + "</span>") +
      row("Local server", i.serverError ? esc(i.serverError) : "127.0.0.1:" + (i.port || 47821) + " · answers iCUE only",
        '<span class="pill ' + (i.serverError ? "danger" : "live") + '">' + (i.serverError ? "Error" : "Running") + "</span>") +
      '<div class="row"><div class="row-text"><span>Install or update widgets</span><small>Packages, formats and steps</small></div><button type="button" class="btn" data-go="widgets">Widgets</button></div></div>';
  }

  function overview(v) {
    return '<div class="page"><h1>Overview</h1><p class="sub">What the companion tracks on this computer and sends to your XENEON EDGE and pump.</p>' +
      (v.paused ? alert("Collection is paused: AI figures stay frozen until you resume.", "warn page-alert") : "") +
      '<div class="group-title">AI usage</div><div class="cards">' + providerCard(v.claude) + providerCard(v.codex) + "</div>" +
      '<div class="group-title">Media</div><div class="cards">' + nowPlayingCard(media) + spotifyCard(sp) + "</div>" +
      '<div class="group-title">Screens</div>' + screensGroup() +
      '<details class="help"' + (helpOpen ? " open" : "") + '><summary>How to read the AI figures</summary><dl>' +
      "<dt>Limits</dt><dd>Share of your plan already used over a period (5 hours, one week). They reset on their own at the time shown.</dd>" +
      "<dt>Tokens today</dt><dd>Amount of text processed since midnight (" + esc(U.TZ) + "). The cache (text already sent and re-read) is often most of it.</dd>" +
      "<dt>Current conversation</dt><dd>Size of the conversation open right now. The bigger it gets, the more each message costs.</dd>" +
      "<dt>Why no link between the two?</dt><dd>Claude and Codex do not say how many tokens one percent of a limit is worth, so they are never converted.</dd>" +
      "</dl></details></div>";
  }

  // ---------- provider page ----------
  function providerPage(p) {
    let limits;
    if (!p.limits.length) limits = '<div class="row">' + alert(p.limitsError || p.reason || "No limits reported.", p.state === "danger" ? "" : "warn") + "</div>";
    else limits = p.limits.map((l) => '<div class="row"><div class="row-text"><span>' + esc(l.name) + "</span><small>Resets " + U.resetText(l.resetAt) +
      (l.stale && l.reason ? " · " + esc(l.reason) : "") + '</small></div><div class="limit-right">' + U.limitBar(l, l.stale ? "off" : "") + "<b>" + l.pct + "% " + l.word + "</b></div></div>").join("");
    const top = U.tightest(p);
    const tr = TREND[p.id] && top ? U.trend(top) : null;
    const eta = tr ? U.forecast(top) : null;
    const trendGroup = tr ? '<div class="group-title">' +  esc(top.name) + " · " + esc(tr.span.toLowerCase()) + "<small>Peak " + tr.peak + '%</small></div><div class="group trend-group">' + tr.svg +
      (eta ? '<p class="hero-meta' + (eta.warn ? " eta-warn" : "") + '">' + esc(eta.text) + "</p>" : "") + "</div>" : "";
    const t = p.tokens;
    const inc = (s) => (t.cacheIncluded ? s + " · included in input" : s);
    const tokenRows = t.total == null ? '<div class="empty">Tokens unavailable: ' + esc(p.name) + " is not detected.</div>" : [
      ["Input", "Text sent to the model (your messages, files, history)", t.input],
      ["Output", "Text generated by the model" + (t.reasoning ? " · including " + U.nf.format(t.reasoning) + " reasoning" : ""), t.output],
      ["Cache read", inc("Text already sent, re-read at lower cost"), t.cacheRead],
      ["Cache write", inc("Text cached for the next messages"), t.cacheWrite],
    ].map(([a, b, n]) => '<div class="row"><div class="row-text"><span>' + a + "</span><small>" + esc(b) + '</small></div><span class="row-value">' + U.nf.format(n) + "</span></div>").join("");
    let ctx;
    if (!p.context) ctx = '<div class="row"><div class="row-text"><span>No open conversation</span><small>Open ' + esc(p.name) + " to see its size here</small></div></div>";
    else if (p.context.capacity) {
      const pct = Math.round(p.context.used / p.context.capacity * 100);
      ctx = '<div class="row"><div class="row-text"><span>Current size · ' + pct + "%</span><small>" + U.big(p.context.used) + " of " + U.big(p.context.capacity) + ' tokens</small></div><div class="limit-right">' + bar(pct) + "<b>" + U.big(p.context.used) + "</b></div></div>";
    } else ctx = '<div class="row"><div class="row-text"><span>Current size</span><small>' + (p.context.estimate ? "Estimate · " : "") + esc(p.name) + ' does not report its maximum capacity</small></div><span class="row-value">' + U.big(p.context.used) + " tokens</span></div>";
    const sessions = p.sessions.length ? p.sessions.map((s) => '<div class="row"><span class="tag">' + esc(s.client) + '</span><div class="row-text"><span>' + esc(s.model) + '</span><small class="mono">' + esc(s.id) + "</small></div>" +
      '<div class="row-text row-right"><span class="num">' + U.big(s.tokens) + " tokens</span><small>" + (s.active ? "active · " : "") + U.ago(s.last) + "</small></div></div>").join("")
      : '<div class="empty">No session in the last 24 hours.</div>';
    const events = p.events.length ? '<div class="group-title">Recent events</div><div class="group">' +
      p.events.slice(0, 6).map((e) => '<div class="row"><div class="row-text"><span>' + esc(e.text) + "</span><small>" + U.ago(e.at) + "</small></div></div>").join("") + "</div>" : "";
    return '<div class="page"><div class="page-head"><span class="mark ' + esc(p.id) + '">' + p.letter + "</span><h1>" + esc(p.name) + "</h1>" + pill(p) + "</div>" +
      '<p class="sub">' + esc(p.clients) + (p.model ? " · " + esc(p.model) : "") + "</p>" +
      (p.reason && p.state !== "live" ? alert(p.reason, p.state === "danger" ? "page-alert" : "warn page-alert") : "") +
      '<div class="group-title">Plan limits<small>Read ' + U.ago(p.limitsAt) + '</small></div><div class="group">' + limits + "</div>" + trendGroup +
      '<div class="group-title">Tokens today<small>Total ' + U.big(t.total) + '</small></div><div class="group">' + tokenRows + "</div>" +
      '<div class="group-title">Current conversation</div><div class="group">' + ctx + "</div>" +
      '<div class="group-title">Recent sessions<small>' + p.sessions.length + ' in 24 h</small></div><div class="group">' + sessions + "</div>" + events +
      '<div class="group-title">Where these figures come from</div><div class="group">' +
      '<div class="row"><div class="row-text"><span>Tokens and conversation</span><small>' + esc(p.src.tokens) + "</small></div></div>" +
      '<div class="row"><div class="row-text"><span>Limits</span><small>' + (p.limits[0] ? "Source: " + (p.id === "claude" ? "Anthropic service (unofficial interface), every minute while in use" : "Codex events, or codex app-server when nothing is recent") : "No reading yet") + "</small></div></div></div></div>";
  }

  // ---------- widgets & settings (need app_info) ----------
  function widgets() {
    const i = info || {};
    const list = i.widgets || [];
    const n = list.filter((w) => w.live).length;
    const state = (w) => w.live ? ["Connected", "live"] : w.placed ? ["Placed · not connected", "warn"] : w.installed ? ["Installed · not placed", ""] : ["Not installed", ""];
    const row = (w) => {
      const [label, cls] = state(w);
      return '<div class="row"><div class="row-text"><span>' + esc(w.name) + "</span><small>" + esc(w.screen) +
        (w.screen === "Pump LCD" ? " · artwork sent through files: the pump renderer cannot open a connection" : "") +
        '</small></div><span class="pill ' + cls + '">' + label + "</span></div>";
    };
    const group = (screen) => list.filter((w) => w.screen === screen).map(row).join("") || '<div class="empty">No widget for this screen.</div>';
    return '<div class="page"><h1>Widgets</h1><p class="sub">Widgets for your XENEON EDGE and your pump LCD (waterblock screen), in iCUE.</p>' +
      (i.serverError ? alert(i.serverError + ". Widgets cannot connect.", "page-alert") : "") +
      '<div class="group-title">XENEON EDGE<small>' + n + " connected in total · iCUE is recognised automatically, no token needed</small></div>" +
      '<div class="group">' + group("XENEON EDGE") + "</div>" +
      '<div class="group-title">Pump LCD</div><div class="group">' + group("Pump LCD") + "</div>" +
      '<div class="group-title">Install a widget</div><div class="group">' +
      '<div class="row"><span class="step">1</span><div class="row-text"><span>Import the .icuewidget package in iCUE</span><small>xeneon-edge folder for the XENEON EDGE, corsair-watercooling folder for the pump</small></div></div>' +
      '<div class="row"><span class="step">2</span><div class="row-text"><span>Place it on the screen</span><small>XENEON EDGE → Widgets, or the pump LCD screen settings</small></div></div>' +
      '<div class="row"><span class="step">3</span><div class="row-text"><span>Keep iCUE Edge Companion running</span><small>It stays quietly in the notification area</small></div></div></div>' +
      '<p class="hint">Read from the iCUE files (installed widgets, XENEON EDGE layout, pump profiles); nothing there is changed.</p></div>';
  }


  function settings(v) {
    const i = info || {};
    const sw = (on, act, label) => '<button type="button" class="switch" role="switch" data-act="' + act + '" aria-checked="' + !!on + '" aria-label="' + label + '"></button>';
    const theme = document.documentElement.dataset.theme;
    const opt = (id, label) => '<button type="button" data-act="theme" data-theme="' + id + '" aria-pressed="' + (theme === id) + '">' + label + "</button>";
    return '<div class="page"><h1>Settings</h1><p class="sub">Appearance, startup, collection and stored data.</p>' +
      '<div class="group-title">Appearance</div><div class="group">' +
      '<div class="row"><div class="row-text"><span>Theme</span><small>OLED keeps every surface true black</small></div><div class="seg" role="group" aria-label="Theme">' +
      opt("light", "Light") + opt("dark", "Dark") + opt("oled", "OLED") + "</div></div></div>" +
      '<div class="group-title">Startup</div><div class="group">' +
      '<div class="row"><div class="row-text"><span>Start with Windows</span><small>iCUE Edge Companion opens in the notification area</small></div>' + sw(i.autostart, "autostart", "Start with Windows") + "</div></div>" +
      '<div class="group-title">Collection</div><div class="group">' +
      '<div class="row"><div class="row-text"><span>Collect data</span><small>Turn off to pause everything</small></div>' + sw(!v.paused, "collect", "Collect data") + "</div>" +
      '<div class="row"><div class="row-text"><span>Refresh limits now</span><small>At most once every 30 seconds</small></div><button type="button" class="btn" data-act="refresh">Refresh</button></div></div>' +
      '<div class="group-title">Data</div><div class="group">' +
      '<div class="row"><div class="row-text"><span>Stored history</span><small>90 days, on this computer only. Nothing is sent elsewhere.</small></div></div>' +
      '<div class="row"><div class="row-text"><span>Clear history</span><small>Ongoing sessions will not be counted again</small></div><button type="button" class="btn danger" data-act="clear">Clear…</button></div></div>' +
      '<div class="group-title">About</div><div class="group">' +
      '<div class="row"><div class="row-text"><span>iCUE Edge Companion ' + esc(i.version || "") + "</span><small>MIT license</small></div></div>" +
      '<div class="row"><div class="row-text"><span>Project</span><small>github.com/inerthel-agi/icue-edge-widgets</small></div><button type="button" class="btn" data-act="link" data-link="repo">Open</button></div>' +
      '<div class="row"><div class="row-text"><span>Author</span><small>github.com/inerthel-agi</small></div><button type="button" class="btn" data-act="link" data-link="profile">Open</button></div>' +
      '<div class="row"><div class="row-text"><small>Not affiliated with or endorsed by Corsair Gaming, Inc., Anthropic, OpenAI or Spotify. iCUE, XENEON, Claude, Codex and Spotify are trademarks of their owners.</small></div></div></div></div>';
  }

  // ---------- Spotify (needs spotify_page; the tokens never reach this page) ----------
  const SP_STATUS = {
    not_configured: ["Not connected", ""], connecting: ["Connecting", "busy"], connected: ["Connected", "live"],
    needs_login: ["Sign in again", "warn"], premium_required: ["Premium required", "danger"], error: ["Action needed", "danger"],
  };
  const validClientId = (v) => /^[A-Za-z0-9]{32}$/.test(v);
  const spRow = (title, sub, right) => '<div class="row"><div class="row-text"><span>' + title + "</span>" + (sub ? "<small>" + sub + "</small>" : "") + "</div>" + (right || "") + "</div>";

  function spotifyPage(s) {
    if (!s) return '<p class="waiting">Loading…</p>';
    const [label, cls] = SP_STATUS[s.status] || SP_STATUS.error;
    const head = '<div class="page-head"><span class="mark spotify">' + U.LOGO.spotify + '</span><h1>Spotify</h1><span class="pill ' + cls + '">' + label + "</span></div>" +
      '<p class="sub">Controls Spotify from the XENEON EDGE widget with your own Spotify app. The companion stores no password and no client secret.</p>';
    const problem = s.message && s.status !== "connected" ? alert(s.message, s.status === "needs_login" ? "warn page-alert" : "page-alert") : "";
    if (s.pending) {
      return '<div class="page">' + head + alert("Finish signing in to Spotify in your browser. This page updates by itself.", "warn page-alert") +
        '<div class="group">' + spRow("Browser did not open?", "Start the sign-in again, or cancel.", '<div class="btns"><button type="button" class="btn" data-act="sp-connect">Open again</button><button type="button" class="btn" data-act="sp-cancel">Cancel</button></div>') + "</div></div>";
    }
    if (s.status === "connected" || (s.status === "connecting" && s.clientIdTail)) {
      const acc = s.account;
      const dev = s.device;
      const item = s.item;
      return '<div class="page">' + head + problem +
        '<div class="group-title">Account</div><div class="group">' +
        spRow(acc ? esc(acc.name) : "Reading account…", acc ? esc(acc.product === "premium" ? "Premium" : acc.product) : "", acc && acc.product === "premium" ? '<span class="tag">Premium</span>' : "") +
        spRow("Client ID", '<span class="mono">' + "•".repeat(28) + esc(s.clientIdTail || "") + "</span>", '<button type="button" class="btn" data-act="sp-change">Change</button>') + "</div>" +
        '<div class="group-title">Access</div><div class="group">' +
        spRow("Token", "Stored encrypted for your Windows account. Refreshed automatically; never displayed.", '<span class="pill ' + (s.status === "connected" ? "live" : "busy") + '">' + (s.status === "connected" ? "Valid" : "Refreshing") + "</span>") +
        spRow("Permissions", '<span class="scopes">' + (s.scopes || []).map((x) => '<span class="tag">' + esc(x) + "</span>").join("") + "</span>") + "</div>" +
        '<div class="group-title">Playback</div><div class="group">' +
        (dev ? spRow(esc(dev.name) + " · " + esc(dev.type), "Active device" + (dev.volume == null ? " · volume fixed by the device" : " · volume " + dev.volume + "%")) : spRow("No active device", "Start Spotify on a computer, phone or speaker.")) +
        (item ? spRow(esc(item.title), esc(item.artists) + (item.playing ? " · playing" : " · paused")) : "") +
        spRow("Lyrics", "Synced lyrics from LRCLIB (community database) · " + esc(s.lyrics)) +
        spRow("Refresh", "Every second while playing, every 3 to 8 s otherwise. Spotify rate limits are respected." + (s.rateLimitedUntil ? " Waiting for Spotify, retry in " + U.dur(s.rateLimitedUntil - Date.now()) + "." : "")) + "</div>" +
        '<div class="group-title">XENEON widget</div><div class="group">' + spRow("Spotify widget", "Add “Spotify” in iCUE · XENEON EDGE. Nothing to enter in iCUE.") + "</div>" +
        '<div class="group">' + spRow("Disconnect", "Deletes the token from this computer. The widget shows “Connect Spotify”.", '<button type="button" class="btn danger" data-act="sp-disconnect">Disconnect</button>') + "</div></div>";
    }
    const again = s.status === "needs_login" && s.clientIdTail
      ? '<div class="group">' + spRow("Connect again", "Uses the saved Client ID ending in " + esc(s.clientIdTail) + ".", '<button type="button" class="btn primary" data-act="sp-connect">Connect again</button>') + "</div>"
      : "";
    return '<div class="page">' + head + problem + again +
      '<div class="group-title">Connect your Spotify account</div><div class="group">' +
      '<div class="row"><ol class="steps">' +
      "<li>Open <b>developer.spotify.com/dashboard</b> and create an app with the <b>Web API</b>.</li>" +
      "<li>Add this <b>Redirect URI</b> to the app, exactly as written.</li>" +
      "<li>Under <b>User Management</b>, add the e-mail of your Spotify account.</li>" +
      "<li>Copy the app's <b>Client ID</b> below, then select <b>Connect</b>. Your browser opens to approve access.</li></ol></div>" +
      spRow("Redirect URI", '<span class="mono">' + esc(s.redirectUri) + "</span>", '<button type="button" class="btn" data-act="sp-copy">Copy</button>') +
      '<div class="row"><div class="row-text grow"><span>Client ID</span><div class="field">' +
      '<input id="sp-client-id" data-act="sp-client-id" autocomplete="off" spellcheck="false" maxlength="32" aria-label="Client ID" placeholder="32 characters from your Spotify app" value="' + esc(spClientId) + '" aria-describedby="sp-hint" />' +
      '<button type="button" class="btn primary" data-act="sp-connect-new"' + (validClientId(spClientId) ? "" : " disabled") + ">Connect</button></div>" +
      '<small class="hint" id="sp-hint">A Client ID is 32 letters and digits. It is not a secret; no client secret is needed.</small></div></div></div>' +
      '<div class="group-title">Requirements</div><div class="group">' +
      spRow("Spotify Premium", "Required by Spotify for playback control, for you and for the app owner.") +
      spRow("Development mode", "A personal Spotify app accepts up to 5 accounts listed under User Management.") + "</div></div>";
  }

  let media = null;
  // The artwork (a data URL) is sent only when the track changes; in between the cached one is reused.
  let mediaArt = { key: null, url: null };
  async function loadMedia() {
    try {
      media = await U.invoke("media_state", { known: mediaArt.key });
      const s = media && media.session;
      if (s && s.art && s.art.url) mediaArt = { key: s.art.key, url: s.art.url };
      else if (s && s.art && s.art.key === mediaArt.key) s.art.url = mediaArt.url;
      else mediaArt = { key: null, url: null };
    } catch (_) { media = null; }
    if (page === "overview") render();
  }

  async function loadSpotify() {
    try { sp = await U.invoke("spotify_page"); } catch (_) { sp = null; }
    render();
  }

  // ---------- rendering ----------
  function render() {
    const v = U.view;
    let html;
    if (!v) html = '<p class="waiting">' + esc(U.link.message || "Loading…") + "</p>";
    else if (page === "claude" || page === "codex") html = providerPage(v[page]);
    else if (page === "widgets") html = widgets();
    else if (page === "spotify") html = spotifyPage(sp);
    else if (page === "settings") html = settings(v);
    else html = overview(v);
    // Re-render only on change so scrolling, focus and open sections stay put.
    if (html !== lastHtml) {
      const focused = document.activeElement && content.contains(document.activeElement) ? document.activeElement.dataset.act || document.activeElement.dataset.go : null;
      const focusedTheme = focused === "theme" ? document.activeElement.dataset.theme : null;
      content.innerHTML = html;
      lastHtml = html;
      U.applyWidths(content);
      if (focused) { const el = content.querySelector(focusedTheme ? '[data-theme="' + focusedTheme + '"]' : '[data-act="' + focused + '"],[data-go="' + focused + '"]'); if (el) el.focus(); }
    }
    chrome(v);
  }

  function chrome(v) {
    const side = document.getElementById("side-status");
    const msg = document.getElementById("footer-msg");
    if (!v) { side.textContent = "Connecting…"; side.classList.add("warn"); return; }
    side.textContent = v.paused ? "Collection paused" : "Collection active";
    side.classList.toggle("warn", v.paused);
    for (const p of [v.claude, v.codex]) {
      document.getElementById("dot-" + p.id).className = "dot " + (p.state === "live" ? "live" : p.state);
      // The dot is colour only: the state is also given to screen readers.
      document.querySelector('.nav[data-page="' + p.id + '"]').setAttribute("aria-label", p.name + ": " + p.stateText);
    }
    if (sp) document.getElementById("dot-spotify").className = "dot " + (sp.status === "connected" ? "live" : sp.status === "not_configured" ? "" : "warn");
    const problem = [v.claude, v.codex].find((p) => p.state === "danger");
    msg.classList.toggle("warn", v.paused || !!problem);
    const last = Math.max(v.claude.lastEventAt || 0, v.codex.lastEventAt || 0, v.claude.limitsAt || 0, v.codex.limitsAt || 0);
    msg.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">' +
      (v.paused ? '<rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/>' : problem ? '<path d="M12 8v5M12 16h.01"/>' : '<path d="M20 6 9 17l-5-5"/>') + "</svg>" +
      esc(v.paused ? "Collection paused" : problem ? problem.name + ": something needs attention" : "Data up to date · last activity " + U.ago(last));
  }

  async function loadInfo() {
    try { info = await U.invoke("app_info"); } catch (_) { info = null; }
    const ver = document.getElementById("version");
    if (info) ver.textContent = "Version " + info.version;
    render();
  }

  function go(next) {
    page = next;
    document.querySelectorAll(".nav").forEach((n) => n.setAttribute("aria-current", n.dataset.page === page ? "page" : "false"));
    lastHtml = "";
    render();
    content.scrollTop = 0;
    if (page === "widgets" || page === "settings") loadInfo();
    if (page === "spotify") loadSpotify();
    if (page === "overview") { loadMedia(); loadInfo(); }
  }

  function flash(el, text, ms) {
    // The first label is kept, so a second click during the message cannot make it permanent.
    const old = (el.dataset.orig ??= el.textContent);
    const btn = el.closest("button") || el;
    el.textContent = text;
    btn.disabled = true;
    clearTimeout(el._flash);
    el._flash = setTimeout(() => { el.textContent = old; btn.disabled = false; }, ms || 1800);
  }

  document.addEventListener("click", async (e) => {
    const win = e.target.closest("[data-win]");
    if (win) {
      const w = window.__TAURI__.window.getCurrentWindow();
      return win.dataset.win === "minimize" ? w.minimize() : win.dataset.win === "maximize" ? w.toggleMaximize() : w.close();
    }
    const nav = e.target.closest("[data-page]"); if (nav) return go(nav.dataset.page);
    const to = e.target.closest("[data-go]"); if (to) return go(to.dataset.go);
    if (e.target.closest("#refresh")) {
      const ok = await U.refresh();
      return flash(document.getElementById("refresh-label"), ok ? "Refresh requested" : "Try again in 30 s");
    }
    const act = e.target.closest("[data-act]");
    if (!act) return;
    switch (act.dataset.act) {
      case "theme": window.setTheme(act.dataset.theme); lastHtml = ""; return render();
      case "autostart":
        await U.invoke("set_autostart", { enabled: act.getAttribute("aria-checked") !== "true" });
        return loadInfo();
      case "collect":
        return U.invoke("set_paused", { paused: act.getAttribute("aria-checked") === "true" });
      case "refresh": return flash(act, (await U.refresh()) ? "Requested" : "In 30 s");
      case "clear":
        if (!confirm("Clear usage history? Today's totals restart from zero; nothing is counted again.")) return;
        await U.invoke("clear_history");
        return flash(act, "Cleared");
      case "link":
        try { await U.invoke("open_link", { link: act.dataset.link }); } catch (_) { flash(act, "Unavailable"); }
        return;
      case "sp-copy":
        try { await navigator.clipboard.writeText(sp.redirectUri); flash(act, "Copied"); } catch (_) { flash(act, "Select and copy it"); }
        return;
      case "sp-connect-new":
      case "sp-connect":
        try { await U.invoke("spotify_connect", { clientId: act.dataset.act === "sp-connect-new" ? spClientId : "" }); }
        catch (err) { window.alert(String(err)); }
        return loadSpotify();
      case "sp-cancel": await U.invoke("spotify_cancel"); return loadSpotify();
      case "sp-change":
      case "sp-disconnect":
        if (!confirm("Disconnect Spotify? The token is deleted from this computer and the widget stops.")) return;
        await U.invoke("spotify_disconnect");
        spClientId = "";
        return loadSpotify();
    }
  });
  // Typing a Client ID only toggles the button: the page is not re-rendered under the caret.
  content.addEventListener("input", (e) => {
    if (e.target.id !== "sp-client-id") return;
    spClientId = e.target.value.trim();
    const ok = validClientId(spClientId);
    e.target.setAttribute("aria-invalid", String(!!spClientId && !ok));
    const hint = document.getElementById("sp-hint");
    if (hint) hint.className = "hint" + (spClientId && !ok ? " bad" : "");
    const btn = content.querySelector('[data-act="sp-connect-new"]');
    if (btn) btn.disabled = !ok;
    // The DOM already shows what was typed: record it, so the next render does not rebuild the field under the caret.
    lastHtml = spotifyPage(sp);
  });
  window.__TAURI__.event.listen("spotify-state", (e) => { sp = e.payload; if (page === "spotify") render(); else chrome(U.view); });
  content.addEventListener("toggle", (e) => { if (e.target.matches("details.help")) helpOpen = e.target.open; }, true);

  document.querySelectorAll("[data-logo]").forEach((el) => { el.innerHTML = U.LOGO[el.dataset.logo]; });
  U.start(render);
  loadInfo();
  loadSpotify();
  // Connected widget count and autostart can change outside this window.
  setInterval(() => { if (page === "widgets" || page === "settings" || page === "overview") loadInfo(); }, 5000);
  setInterval(() => { if (page === "overview") loadMedia(); }, 2000);
  loadMedia();
})();
