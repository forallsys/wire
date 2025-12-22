---
# https://vitepress.dev/reference/default-theme-home-page
layout: home

hero:
  name: wire
  text: a tool to deploy nixos systems
  actions:
    - theme: brand
      text: Read Tutorial
      link: /tutorial/overview
    - theme: alt
      text: How-to Guides
      link: /guides/installation
    - theme: alt
      text: Sources
      link: https://github.com/forallsys/wire.git

features:
  - title: Deploy in Parallel
    details: Build and deploy many nodes at once.
    link: /guides/parallelism
    icon: 💽
  - title: Manage Secrets
    details: Fast & Unopinionated secret management. Bring your own GPG, Age, or any other encryption tool.
    link: /guides/keys
    icon: 🔑
  - title: Deploy as Any User
    details: Non-root deployments and interactive authentication is fully supported.
    link: /guides/non-root-user
    icon: 🧑‍💻
---
