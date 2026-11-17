// Vite empties dist/ on every build. dist/.gitkeep is tracked (an empty file) so that the
// directory always exists in a checkout: the server's build script watches it. Re-create it.
import { writeFileSync } from "node:fs";

writeFileSync(new URL("../dist/.gitkeep", import.meta.url), "");
