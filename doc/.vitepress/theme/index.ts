import DefaultTheme from "vitepress/theme";
import "virtual:group-icons.css";
import { EnhanceAppContext } from "vitepress";
import "./style.css";

export default {
  ...DefaultTheme,
  enhanceApp(ctx: EnhanceAppContext) {
    DefaultTheme.enhanceApp(ctx);
  },
};
