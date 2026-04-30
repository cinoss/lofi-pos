import type { LinguiConfig } from "@lingui/conf";
import { formatter } from "@lingui/format-po";

const config: LinguiConfig = {
  sourceLocale: "en",
  locales: ["en", "vi"],
  catalogs: [
    {
      path: "<rootDir>/src/locales/{locale}/messages",
      include: [
        "<rootDir>/src",
        "<rootDir>/../../packages/pos-ui/src",
        "<rootDir>/../../packages/ui/src",
      ],
    },
  ],
  format: formatter({ lineNumbers: false }),
};

export default config;
