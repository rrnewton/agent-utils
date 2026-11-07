// app (`#624 voice-page-modularity`, plan step 17): the composition root — the ordered boot
// and, in the final entry step, the page's construction itself. Its public interface.

export { createApplication } from "./application.ts";
export type { Application, ApplicationPorts, MountThunk, Mounted, StartAct } from "./application.ts";
