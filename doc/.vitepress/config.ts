import { defineConfig } from "vitepress";
import pkg from "../package.json";
import markdownItFootnote from "markdown-it-footnote";
import {
  groupIconMdPlugin,
  groupIconVitePlugin,
  localIconLoader,
} from "vitepress-plugin-group-icons";

const MODE = (process.env.MODE ?? "unstable") as "unstable" | "stable";

// https://vitepress.dev/reference/site-config
export default defineConfig({
  title: MODE === "stable" ? "wire" : "wire (unstable!)",
  description: "a tool to deploy nixos systems",
  themeConfig: {
    search: {
      provider: "local",
    },

    footer: {
      message:
        'Released under the <a href="https://github.com/forallsys/wire/blob/trunk/COPYING">AGPL-3.0 License</a>.',
      copyright: "Copyright 2024-2025 wire Contributors",
    },

    // https://vitepress.dev/reference/default-theme-config
    nav: [
      { text: "Home", link: "/" },
      { text: "Tutorial", link: "/tutorial/overview" },
      { text: "Guides", link: "/guides/installation" },
      { text: "Reference", link: "/reference/cli" },
      {
        text: MODE === "stable" ? pkg.version : `Unstable (${pkg.version})`,
        items: [
          MODE === "unstable"
            ? {
                text: `View Stable`,
                link: "https://wire.forall.systems",
              }
            : {
                text: "View Unstable",
                link: "https://trunk.wire-docs.pages.dev",
              },
          {
            text: "Changelog",
            link: "https://github.com/forallsys/wire/blob/trunk/CHANGELOG.md",
          },
          {
            text: "CI Server",
            link: "https://buildbot.althaea.zone/#/projects/1",
          },
        ],
      },
    ],

    sidebar: {
      "/": [
        {
          text: "Tutorial",
          collapsed: false,
          items: [
            { text: "Overview", link: "/tutorial/overview" },
            {
              text: "Part One",
              items: [
                {
                  text: "Nix Setup",
                  link: "/tutorial/part-one/nix-setup",
                },
                {
                  text: "Preparing Repo & Shell",
                  link: "/tutorial/part-one/repo-setup",
                },
                {
                  text: "Creating a Virtual Machine",
                  link: "/tutorial/part-one/vm-setup",
                },
                {
                  text: "Basic Hive & Deployment",
                  link: "/tutorial/part-one/basic-hive",
                },
              ],
            },
            {
              text: "Part Two",
              items: [
                {
                  text: "Basic Deployment Keys",
                  link: "/tutorial/part-two/basic-keys",
                },
                {
                  text: "Encrypted Deployment Keys",
                  link: "/tutorial/part-two/encryption",
                },
              ],
            },
          ],
        },
        {
          text: "How-To Guides",
          collapsed: false,
          items: [
            { text: "Install wire", link: "/guides/installation" },
            {
              text: "Write a Hive",
              link: "/guides/writing-a-hive",
            },
            { text: "Migrate to wire", link: "/guides/migrate" },
            {
              text: "Use Flakes",
              link: "/guides/flakes/overview",
              items: [
                {
                  text: "Keep Using nixos-rebuild",
                  link: "/guides/flakes/nixos-rebuild",
                },
              ],
            },
            { text: "Apply your Config", link: "/guides/apply" },
            { text: "Target Nodes", link: "/guides/targeting" },
            { text: "Build in CI", link: "/guides/build-in-ci" },
            {
              text: "Features",
              items: [
                {
                  text: "Use a non-root user",
                  link: "/guides/non-root-user",
                },
                { text: "Manage Secrets", link: "/guides/keys" },
                { text: "Use Parallelism", link: "/guides/parallelism" },
                { text: "Use hive.default", link: "/guides/hive-default" },
              ],
            },
          ],
        },
        { text: "CLI & Module Reference", link: "/reference/cli.html" },
      ],
      "/reference/": [
        {
          text: "Reference",
          items: [
            { text: "CLI", link: "/reference/cli" },
            { text: "Meta Options", link: "/reference/meta" },
            { text: "Module Options", link: "/reference/module" },
          ],
        },
      ],
    },

    editLink: {
      pattern: "https://github.com/forallsys/wire/edit/trunk/doc/:path",
      text: "Edit this page on GitHub",
    },

    socialLinks: [
      { icon: "github", link: "https://github.com/forallsys/wire" },
    ],
  },
  markdown: {
    config: (md) => {
      md.use(markdownItFootnote);
      md.use(groupIconMdPlugin, {
        titleBar: { includeSnippet: true },
      });
    },
  },
  vite: {
    plugins: [
      groupIconVitePlugin({
        customIcon: {
          nixos: "vscode-icons:file-type-nix",
          "configuration.nix": "vscode-icons:file-type-nix",
          "hive.nix": "vscode-icons:file-type-nix",
          "flake.nix": "vscode-icons:file-type-nix",
          "module.nix": "vscode-icons:file-type-nix",
          "vm.nix": "vscode-icons:file-type-nix",
          "shell.nix": "vscode-icons:file-type-nix",
          "secrets.nix": "vscode-icons:file-type-nix",
          home: localIconLoader(import.meta.url, "../assets/homemanager.svg"),
          ".conf": "vscode-icons:file-type-config",
        },
      }),
    ],
  },
});
