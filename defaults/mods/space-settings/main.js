// Per-space settings: startup command. Enter saves, Esc cancels.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { run, props, closeOverlay, theme, appHome, readTextFile, notify } from "herdr";

export default class SpaceSettings extends View {
  init(_props, cx) {
    this.text = props()?.value ?? "";
    this.input = InputState.new({ value: this.text });
    if (this.text) this.input.set_selected_range({ start: 0, end: this.text.length });
    this.input.on("change", (_e, cx) => {
      this.text = this.input.value();
      cx.notify();
    });
    this.input.on("submit", () => this.submit());
    this.inputFocused = true;
    this.input.on("focus", (_e, cx) => {
      this.inputFocused = true;
      cx.notify();
    });
    this.input.on("blur", (_e, cx) => {
      this.inputFocused = false;
      cx.notify();
    });
    this.input.focus();
    cx.timer.after(600, () => this.input.focus());
    this.handle = cx.focus_handle();
    this.focusCancel = cx.focus_handle();
    this.focusSave = cx.focus_handle();
    this.focusRun = cx.focus_handle();
    this.focusRefresh = cx.focus_handle();
    this.runBusy = false;
    this.runNote = null;
    this.focusStamp = "";
    this.last = props()?.last_run && typeof props().last_run === "object" ? props().last_run : null;
    // Focus rings only when focus/busy state changes (not a 200ms paint churn).
    cx.timer.every(500, (cx) => {
      const stamp = [
        this.focusCancel?.is_focused?.(),
        this.focusSave?.is_focused?.(),
        this.focusRun?.is_focused?.(),
        this.focusRefresh?.is_focused?.(),
        this.runBusy,
        this.inputFocused,
      ].join("|");
      if (stamp === this.focusStamp) return;
      this.focusStamp = stamp;
      cx.notify();
    });
  }

  lastRunFile() {
    return `${appHome()}/space-startup-last.json`;
  }

  async loadLastRun() {
    try {
      const raw = await readTextFile(this.lastRunFile());
      const parsed = raw ? JSON.parse(raw) : null;
      const id = props()?.id;
      if (parsed && typeof parsed === "object" && (!id || parsed.space_id === id)) this.last = parsed;
      else if (!parsed) this.last = null;
    } catch {
      this.last = null;
    }
  }

  submit() {
    const p = props();
    closeOverlay(p.overlay);
    run(p.command.name, { ...(p.command.args ?? {}), command: this.text });
  }

  runNow(cx) {
    if (this.runBusy) return;
    const value = this.text;
    if (!String(value || "").trim()) {
      notify({ title: "Write a command first", level: "error" });
      return;
    }
    const p = props() ?? {};
    this.runBusy = true;
    this.runNote = null;
    const beforeStarted = this.last?.started_at_ms ?? null;
    const beforeFinished = this.last?.finished_at_ms ?? null;
    run("space.run_startup_command", { id: p.id, command: value });
    cx.notify();
    cx.spawn(async (cx) => {
      let accepted = false;
      const acceptDeadline = Date.now() + 1200;
      while (Date.now() < acceptDeadline) {
        await this.loadLastRun();
        if (this.last?.started_at_ms != null && this.last.started_at_ms !== beforeStarted) {
          accepted = true;
          break;
        }
        await cx.sleep(100);
      }
      if (!accepted) {
        this.runBusy = false;
        this.runNote = "Did not start — refused or already failing";
        cx.notify();
        return;
      }
      const deadline = Date.now() + 130000;
      let done = false;
      while (Date.now() < deadline) {
        await this.loadLastRun();
        const finished = this.last?.finished_at_ms;
        const started = this.last?.started_at_ms;
        if (finished && finished !== 0 && (finished !== beforeFinished || started !== beforeStarted)) {
          done = true;
          break;
        }
        await cx.sleep(400);
      }
      this.runBusy = false;
      if (!done) this.runNote = "Still running — use Refresh last run";
      else this.runNote = null;
      cx.notify();
    });
  }

  key(e, cx) {
    if (e.key === "escape") {
      closeOverlay(props().overlay);
      cx.stop_propagation();
    }
  }

  wrapPreview(text) {
    return text
      .split("\n")
      .slice(0, 10)
      .flatMap((l) => {
        const chunks = [];
        for (let i = 0; i < Math.max(l.length, 1); i += 96) chunks.push(l.slice(i, i + 96) || " ");
        return chunks;
      })
      .join("\n");
  }

  lastRunSummary(t, last) {
    if (!last && !this.runBusy && !this.runNote) return null;
    const when = last?.finished_at_ms
      ? new Date(last.finished_at_ms).toLocaleString()
      : null;
    let status = "Last run recorded.";
    if (this.runBusy) status = "Running…";
    else if (this.runNote) status = this.runNote;
    else if (last && (last.finished_at_ms === 0 || last.finished_at_ms == null) && last.error === "Running…") status = "Running…";
    else if (last?.timed_out) status = "Timed out";
    else if (last?.error) status = last.error;
    else if (last?.exit_code === 0) status = "Succeeded";
    else if (last) status = `Failed (exit ${last.exit_code ?? "?"})`;
    const detail = last ? [last.stderr, last.stdout, last.error === "Running…" ? "" : last.error].filter(Boolean).join("\n").trim() : "";
    const preview = detail ? this.wrapPreview(detail) : "";
    const ok = last && last.exit_code === 0 && !last.timed_out && !last.error;
    return v_flex()
      .pt(12)
      .gap(4)
      .child(div().text_size(11.5).text_color(t.dim).child("Last run"))
      .child(div().text_size(12.5).text_color(ok && !this.runBusy && !this.runNote ? t.foreground : t.blocked).child(`${status}${when && !this.runBusy ? ` · ${when}` : ""}`))
      .when(Boolean(preview) && !this.runBusy, (el) =>
        el.child(
          div()
            .mt(4)
            .p(8)
            .rounded(8)
            .border_1()
            .border_color(t.border)
            .bg(t.background)
            .text_size(11.5)
            .text_color(t.muted)
            .whitespace_normal()
            .child(preview),
        ),
      );
  }

  render(cx) {
    const t = theme();
    const p = props() ?? {};
    const title = (p.title ?? "Space").replace(/^./, (c) => c.toUpperCase());
    const last = this.last;
    const label = (text) => div().pt(14).pb(6).text_size(11.5).text_color(t.dim).child(text);
    const button = (id, text, primary, action, focus) => {
      const on = focus?.is_focused?.() === true;
      let el = h_flex()
        .id(id)
        .h(36)
        .min_h(36)
        .px(14)
        .items_center()
        .rounded(8)
        .border_1()
        .border_color(on ? t.accent : primary ? t.solid : t.input)
        .bg(primary ? t.solid : t.raised)
        .text_color(primary ? t.solid_foreground : t.foreground)
        .text_size(12.5)
        .font_medium()
        .cursor_pointer()
        .hover((s) => s.opacity(0.88))
        .on_click(action)
        .child(text);
      if (focus) {
        el = el
          .track_focus(focus)
          .tab_stop(true)
          .on_key_down((e, cx) => {
            if (e.key === "enter" || e.key === " ") {
              action(e, cx);
              cx.stop_propagation();
            }
          });
      }
      return el;
    };
    return v_flex()
      .id("space-settings")
      .track_focus(this.handle)
      .on_key_down((e, cx) => this.key(e, cx))
      .w_full()
      .min_h(0)
      .p(20)
      .gap(2)
      .bg(t.popover)
      .text_color(t.foreground)
      .child(
        h_flex()
          .items_center()
          .child(div().flex_1().text_size(15).font_semibold().child(title))
          .child(div().text_size(11).text_color(t.dim).child("Esc")),
      )
      .child(label("Startup command"))
      .child(
        Input.new(this.input)
          .h(36)
          .px(10)
          .rounded(9)
          .border_1()
          .border_color(this.inputFocused ? t.accent : t.input)
          .bg(t.background)
          .text_size(12.8)
          .text_color(t.foreground),
      )
      .child(
        div()
          .pt(6)
          .text_size(11.5)
          .text_color(t.dim)
          .child("Runs once when this space opens (applies on next open). Empty: do nothing."),
      )
      .when(Boolean(last || this.runBusy || this.runNote), (el) => el.child(this.lastRunSummary(t, last)))
      .child(
        h_flex()
          .pt(16)
          .gap(8)
          .justify_end()
          .flex_wrap()
          .child(button("ss-run", this.runBusy ? "Running…" : "Run now", false, (_e, cx) => this.runNow(cx), this.focusRun))
          .child(button("ss-refresh", "Refresh last run", false, (_e, cx) => {
            cx.spawn(async (cx) => {
              await this.loadLastRun();
              this.runNote = null;
              cx.notify();
            });
          }, this.focusRefresh))
          .child(button("ss-cancel", "Cancel", false, () => closeOverlay(p.overlay), this.focusCancel))
          .child(button("ss-save", "Save", true, () => this.submit(), this.focusSave)),
      );
  }
}
