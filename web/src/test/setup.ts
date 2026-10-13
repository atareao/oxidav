import '@testing-library/jest-dom/vitest'

// jsdom does not implement ResizeObserver, which several Ant Design components
// (Select, Modal, Popconfirm, … via rc-resize-observer) rely on.
if (typeof window.ResizeObserver === 'undefined') {
  class ResizeObserverStub implements ResizeObserver {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  window.ResizeObserver = ResizeObserverStub
}

// jsdom does not implement matchMedia, which Ant Design's responsive
// components (e.g. the Layout.Sider breakpoint) rely on.
if (!window.matchMedia) {
  window.matchMedia = (query: string): MediaQueryList =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList
}
