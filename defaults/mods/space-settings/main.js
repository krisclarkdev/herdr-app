// Per-space settings: startup command. Enter saves, Esc cancels.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { run, props, closeOverlay, theme } from "herdr";

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
  }

  submit() {
    const p = props();
    closeOverlay(p.overlay);
    run(p.command.name, { ...(p.command.args ?? {}), command: this.text });
  }

  key(e, cx) {
    if (e.key === "escape") {
      closeOverlay(props().overlay);
      cx.stop_propagation();
    }
  }

  lastRunSummary(t, last) {
    if (!last) return null;
    const when = last.finished_at_ms
      ? new Date(last.finished_at_ms).toLocaleString()
      : null;
    let status = "Last run recorded.";
    if (last.timed_out) status = "Timed out";
    else if (last.error) status = last.error;
    else if (last.exit_code === 0) status = "Succeeded";
    else status = `Failed (exit ${last.exit_code ?? "?"})`;
    const detail = [last.stderr, last.stdout, last.error].filter(Boolean).join("\n").trim();
    const preview = detail
      ? detail.split("\n").slice(0, 8).map((l) => (l.length > 120 ? `${l.slice(0, 117)}…` : l)).join("\n")
      : "";
    return v_flex()
      .pt(14)
      .gap(4)
      .child(div().text_size(11.5).text_color(t.dim).child("Last run"))
      .child(div().text_size(12.5).text_color(last.exit_code === 0 && !last.timed_out && !last.error ? t.foreground : t.blocked).child(`${status}${when ? ` · ${when}` : ""}`))
      .when(Boolean(preview), (el) =>
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
            .child(preview),
        ),
      );
  }

  render() {
    const t = theme();
    const p = props() ?? {};
    const title = (p.title ?? "Space").replace(/^./, (c) => c.toUpperCase());
    const last = p.last_run && typeof p.last_run === "object" ? p.last_run : null;
    const label = (text) => div().pt(16).pb(7).text_size(11.5).text_color(t.dim).child(text);
    const button = (id, text, primary, action) =>
      h_flex()
        .id(id)
        .h(32)
        .px(14)
        .items_center()
        .rounded(8)
        .border_1()
        .border_color(primary ? t.solid : t.input)
        .bg(primary ? t.solid : t.raised)
        .text_color(primary ? t.solid_foreground : t.foreground)
        .text_size(12.5)
        .font_medium()
        .cursor_pointer()
        .hover((s) => s.opacity(0.88))
        .on_click(action)
        .child(text);
    return v_flex()
      .id("space-settings")
      .track_focus(this.handle)
      .on_key_down((e, cx) => this.key(e, cx))
      .w_full()
      .min_h(0)
      .p(18)
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
          .child("Runs once when this space opens. Empty: do nothing."),
      )
      .when(Boolean(last), (el) => el.child(this.lastRunSummary(t, last)))
      .child(
        h_flex()
          .pt(20)
          .gap(8)
          .justify_end()
          .child(button("ss-cancel", "Cancel", false, () => closeOverlay(p.overlay)))
          .child(button("ss-save", "Save", true, () => this.submit())),
      );
  }
}
