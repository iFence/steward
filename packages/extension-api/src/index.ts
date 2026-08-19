/**
 * Steward plugin extension API — M2 草案。
 *
 * 这里定义插件可见的最小 API 面（`List` / `ActionPanel` / `showToast`）。
 * 运行时实现（宿主函数注入）在 M2 里程碑落地；当前仅提供类型契约与版本号。
 */

export const version = "0.1.0";

export interface ListItem {
  id: string;
  title: string;
  subtitle?: string;
  keywords?: string[];
  icon?: string;
}

export interface ListOptions {
  items: ListItem[];
  onSelect?: (item: ListItem) => void;
}

/** 在 Steward UI 中渲染一个可搜索的列表。 */
export function List(options: ListOptions): void {
  // TODO(M2): 由插件宿主运行时实现。
  void options;
}

export interface Action {
  id: string;
  title: string;
  onRun: () => void | Promise<void>;
}

export interface ActionPanelOptions {
  actions: Action[];
}

/** 渲染操作面板（操作列表 / 详情视图）。 */
export function ActionPanel(options: ActionPanelOptions): void {
  // TODO(M2): 由插件宿主运行时实现。
  void options;
}

export interface ToastOptions {
  message: string;
  kind?: "info" | "success" | "error";
  durationMs?: number;
}

/** 在 Steward UI 中显示一条短暂的通知。 */
export function showToast(options: ToastOptions): void {
  // TODO(M2): 由插件宿主运行时实现。
  void options;
}
