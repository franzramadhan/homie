// The parts of noVNC's RFB (core/rfb.js) the console page uses; noVNC ships no types.
declare module "@novnc/novnc" {
  export default class RFB extends EventTarget {
    constructor(target: HTMLElement, url: string, options?: { credentials?: { password?: string }; wsProtocols?: string[] });
    scaleViewport: boolean;
    disconnect(): void;
    sendCtrlAltDel(): void;
    sendCredentials(credentials: { password?: string }): void;
  }
}
