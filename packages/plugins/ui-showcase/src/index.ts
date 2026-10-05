import { button, col, input, row, text, ui } from "@steward/extension-api";
import type { ElementEvent, UiView } from "@steward/extension-api";

let clicks = 0;
let query = "";

function render(): UiView {
  return ui(
    col()
      .w_full()
      .gap(8)
      .p(12)
      .child(text("UI Showcase").text_color("primary").text_lg().font_weight("semibold"))
      .child(text(`Clicks: ${clicks}`).text_color("muted_foreground").text_sm())
      .child(
        row()
          .gap(8)
          .items_center()
          .child(
            button("Increment")
              .px(12)
              .py(4)
              .rounded_md()
              .onClick(() => {
                clicks += 1;
                return render();
              }),
          )
          .child(
            input("q", { placeholder: "Type here", value: query }).onChange(
              (event: ElementEvent) => {
                query = event.value ?? "";
                return render();
              },
            ),
          ),
      )
      .child(
        text(query.length > 0 ? `Query: ${query}` : "Type in the box")
          .text_color("muted_foreground")
          .text_sm(),
      ),
  );
}

export function command(): UiView {
  return render();
}
