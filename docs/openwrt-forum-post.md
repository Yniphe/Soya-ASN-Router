# Draft: OpenWrt Forum / Reddit Announcement

Title:

```text
Soya ASN Router: LuCI app for ASN-based split routing, WireGuard tunnel groups, and custom IPv4 overrides
```

Post:

````markdown
I built a small OpenWrt package for ASN-based policy routing:

https://github.com/Yniphe/Soya-ASN-Router

The goal is to route traffic for selected internet services through a chosen
WAN, WireGuard tunnel, or ordered tunnel failover group without maintaining big
IP lists manually.

What it does:

- configure ASNs from LuCI, for example Google, AWS, Netflix, Meta, Cloudflare;
- fetch announced IPv4 prefixes from RIPEstat and store them locally in SQLite;
- generate LAN-only nftables + Linux policy routing rules;
- select a target interface per ASN;
- use interface groups like `wg0`, `wg1`, `wg2` with health checks and failover;
- add custom IPv4/CIDR route overrides for exact exceptions;
- preview route policy before applying it;
- pause/resume generated route policy from LuCI.

Typical use cases:

- split traffic between WAN and WireGuard by provider ASN;
- keep media or cloud traffic on a specific tunnel;
- route a broad ASN through VPN but override one IP/network back to WAN;
- fail over between multiple WireGuard tunnels automatically.

Current release:

https://github.com/Yniphe/Soya-ASN-Router/releases/tag/v0.1.0-r15

The release includes packages for:

- OpenWrt 24.10.4 mediatek/filogic (`.ipk`);
- OpenWrt 25.12.2 mediatek/filogic (`.apk`).

OpenWrt 25.12.2 quick install example:

```sh
cd /tmp

wget -O soya-asn-router-0.1.0-r15.apk \
  https://github.com/Yniphe/Soya-ASN-Router/releases/download/v0.1.0-r15/openwrt-25.12.2-mediatek-filogic-soya-asn-router-0.1.0-r15.apk

wget -O luci-app-soya-asn-router-r15.apk \
  https://github.com/Yniphe/Soya-ASN-Router/releases/download/v0.1.0-r15/openwrt-25.12.2-mediatek-filogic-luci-app-soya-asn-router-26.136.52279.79b8e2d.apk

apk add --allow-untrusted --force-overwrite \
  /tmp/soya-asn-router-0.1.0-r15.apk \
  /tmp/luci-app-soya-asn-router-r15.apk

rm -rf /tmp/luci-indexcache /tmp/luci-modulecache
/etc/init.d/rpcd reload
/etc/init.d/uhttpd reload
/etc/init.d/soya-asn-router enable
/etc/init.d/soya-asn-router restart
```

LuCI page:

```text
Services -> Soya ASN Router
```

ASN preset import URL:

```text
https://github.com/Yniphe/Soya-ASN-Router/releases/download/v0.1.0-r15/soya-asn-router-asns.txt
```

This is not a full replacement for every PBR setup. It is focused on router-side
IPv4 routing by ASN and explicit IPv4/CIDR overrides. Domain-based policy is
intentionally out of scope for now.

Feedback, target requests, and bug reports are welcome.
````
