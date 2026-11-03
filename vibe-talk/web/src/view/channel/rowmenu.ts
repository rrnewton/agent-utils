// The one row ⋯ menu that is open (`#202 read-through-here`; `#624 voice-page-modularity`, moved from the
// legacy script into view/channel). ONE menu open at a time: opening one closes whichever was open, as a
// phone's own pop-up menus do. Building the menus, and what closes them (a tap outside, Escape), are the
// page's.

/** An open ⋯ menu: its root, what closes it, and the ⋯ that opened it. */
export interface RowMenu {
  readonly root: Element;
  readonly close: () => void;
  readonly button: HTMLElement;
}

/** The menus' commands and questions. */
export interface RowMenus {
  /** The menu open now, or null. */
  open(): RowMenu | null;
  /** Menu `menu` has opened: whichever other was open is closed first. */
  opened(menu: RowMenu): void;
  /** The menu closed by `close` has closed: none is open, where it was the one. */
  closed(close: () => void): void;
}

/** No menu open. */
export function createRowMenus(): RowMenus {
  let openRowMenu: RowMenu | null = null;

  return {
    open: () => openRowMenu,
    opened(menu) {
      if (openRowMenu && openRowMenu.close !== menu.close) openRowMenu.close();
      openRowMenu = menu;
    },
    closed(close) {
      if (openRowMenu && openRowMenu.close === close) openRowMenu = null;
    },
  };
}
