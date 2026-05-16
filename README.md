# Soya ASN Router for OpenWrt

[![Build OpenWrt Packages](https://github.com/Yniphe/Soya-ASN-Router/actions/workflows/openwrt-ipk.yml/badge.svg)](https://github.com/Yniphe/Soya-ASN-Router/actions/workflows/openwrt-ipk.yml)

Route traffic for selected internet services through the right OpenWrt
interface, VPN tunnel, or tunnel failover group, using ASNs plus optional exact
IPv4/CIDR overrides instead of manually maintained IP lists.

Soya ASN Router is an OpenWrt package with a LuCI app and a small Rust backend.
It fetches announced prefixes for configured ASNs from RIPEstat, stores them in
SQLite, and generates LAN-only IPv4 route policies with nftables marks and Linux
policy routing.

Keywords: OpenWrt ASN routing, policy based routing, PBR alternative,
WireGuard split tunneling, LuCI routing, nftables policy routing, VPN failover.

## Why Use It

Use Soya ASN Router when you want router-level policy routing for whole
providers or services, without copying huge prefix lists by hand.

Typical examples:

- route Google, AWS, Netflix, Meta, Cloudflare, or other provider networks
  through a selected WAN or WireGuard interface;
- keep corporate SaaS, cloud, or media traffic on a dedicated tunnel;
- override specific IP addresses or networks when ASN-level routing is too
  broad;
- bulk import and manage many ASNs from a plain text URL;
- preview route policy changes before applying them;
- pause/resume generated routing policy from LuCI;
- define ordered tunnel groups like `wg0`, `wg1`, `wg2` and automatically move
  traffic to the next healthy tunnel when the active one fails.

The router does not need client-side configuration. Matching is done on the
router for LAN-originated IPv4 traffic.

## Highlights

- LuCI page under `Services -> Soya ASN Router`.
- Per-ASN target interface selection.
- ASN import from HTTP/HTTPS URL.
- Bulk ASN delete and bulk target interface changes.
- Custom IPv4/CIDR route overrides.
- RIPEstat ASN holder names in the status table.
- Sync missing ASNs or force-sync all ASNs.
- Periodic background synchronization.
- Route policy preview before applying.
- Pause/resume route policy without deleting configuration.
- Interface failover groups with editable health-check URL.
- Default health-check URL: `https://www.google.com/generate_204`.
- Persistent SQLite database under `/etc/soya-asn-router/soya.db`.
- Sysupgrade keep entry for `/etc/soya-asn-router/`.

## How It Works

1. You configure ASNs such as `AS15169` or `15169` in LuCI.
2. The backend fetches announced prefixes from RIPEstat and stores them locally.
3. Optional custom IPv4 routes such as `8.8.8.8/32` can be added for precise
   overrides.
4. Each ASN or custom route points to either an OpenWrt interface such as `wan`
   or `wg0`, or to an interface group such as `group:vpn_main`.
5. The backend generates:

   ```sh
   /etc/soya-asn-router/routes.nft
   /etc/soya-asn-router/routes.sh
   ```

6. nftables marks LAN IPv4 traffic whose destination matches ASN prefixes or
   custom routes.
7. Linux policy routing sends marked traffic through the selected target
   interface.
8. On daemon startup, enabled route policy is reconciled again, so it survives
   router reboot.

Route policy is grouped by active target interface: one nft set and one
fwmark/routing table pair are generated per target.

Custom IPv4 routes are evaluated after ASN prefix rules. If a custom route
overlaps an ASN prefix and points to a different target, the custom route wins.

## Interface Groups And Tunnel Failover

Interface groups let you route ASNs to a logical target instead of one fixed
tunnel.

Example:

```text
vpn_main
  1. wg0
  2. wg1
  3. wg2
```

In an ASN row, select `Group: vpn_main`. Internally this is stored as:

```text
group:vpn_main
```

The daemon checks group members with `curl --interface <device> --ipv4` against
the configured health-check URL. After the failure threshold is reached, the
active interface moves to the next healthy tunnel. If `Prefer primary interface`
is enabled, the group returns to the first tunnel after the recovery threshold is
met.

If every tunnel in a group is unhealthy, Soya ASN Router keeps the last active
interface and reports the group as unhealthy. It does not silently fall back to
WAN, which avoids accidental VPN traffic leaks.

## Custom IPv4 Route Overrides

Use custom routes for individual addresses or networks that are not convenient
to express with ASNs.

Examples:

```text
8.8.8.8/32      -> wan
203.0.113.0/24  -> group:vpn_main
```

Destinations accept IPv4 addresses and CIDR networks. A plain IPv4 address is
treated as `/32`. Domain names are intentionally not resolved in this feature;
keep DNS/domain-based policy in a separate tool when you need it.

## Install From A Release

Use the package set matching your router's OpenWrt release and target.

Check your router:

```sh
ubus call system board
```

Look at:

- `release.version`, for example `24.10.4`;
- `release.target`, for example `mediatek/filogic`.

Download the matching assets from the GitHub Release.

Quick install for OpenWrt `25.12.2` on `mediatek/filogic`:

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

ASN preset import URL:

```text
https://github.com/Yniphe/Soya-ASN-Router/releases/download/v0.1.0-r15/soya-asn-router-asns.txt
```

For OpenWrt `24.10.x` release assets:

```sh
scp openwrt-24.10.4-mediatek-filogic-*.ipk root@192.168.1.1:/tmp/
ssh root@192.168.1.1
opkg install \
  /tmp/openwrt-24.10.4-mediatek-filogic-soya-asn-router_*.ipk \
  /tmp/openwrt-24.10.4-mediatek-filogic-luci-app-soya-asn-router_*.ipk
/etc/init.d/rpcd reload
/etc/init.d/uhttpd reload
/etc/init.d/soya-asn-router enable
/etc/init.d/soya-asn-router restart
```

For OpenWrt `25.x` `.apk` release assets:

```sh
scp openwrt-25.12.2-mediatek-filogic-*.apk root@192.168.1.1:/tmp/
ssh root@192.168.1.1
apk add --allow-untrusted \
  /tmp/openwrt-25.12.2-mediatek-filogic-soya-asn-router-*.apk \
  /tmp/openwrt-25.12.2-mediatek-filogic-luci-app-soya-asn-router-*.apk
/etc/init.d/rpcd reload
/etc/init.d/uhttpd reload
/etc/init.d/soya-asn-router enable
/etc/init.d/soya-asn-router restart
```

Then open LuCI:

```text
Services -> Soya ASN Router
```

## First Run

1. Enable the backend daemon.
2. Add ASNs manually or import a comma/whitespace-separated ASN list from a URL.
3. Add custom IPv4 routes for exact overrides if needed.
4. Select a target interface or interface group for each ASN and custom route.
5. Run `Synchronize missing` or `Synchronize all`.
6. Open `Preview route policies` and verify the generated target groups.
7. Apply route policies.

The status tables show synchronization progress, per-ASN prefix counts, route
policy state, and interface group health.

## Runtime Commands

Useful commands on the router:

```sh
soya-asn-router status
soya-asn-router interfaces
soya-asn-router sync-missing
soya-asn-router sync-all
soya-asn-router preview-routes
soya-asn-router check-groups
soya-asn-router generate-routes
soya-asn-router apply-routes
soya-asn-router pause-routes
soya-asn-router resume-routes
soya-asn-router dedupe-config
soya-asn-router import-url https://example.com/asns.txt wg0
soya-asn-router bulk-delete "AS15169 AS32934"
soya-asn-router bulk-set-interface "AS15169 AS32934" group:vpn_main
```

Logs:

```sh
logread -f | grep soya-asn-router
```

## Build With An OpenWrt SDK

Use the SDK or buildroot for the same OpenWrt release and target as your router.

From inside the extracted SDK/buildroot:

```sh
echo "src-link soya /absolute/path/to/Soya-ASN-Router" >> feeds.conf.default
sed -i -E 's#^src-git-full base https://git.openwrt.org/openwrt/openwrt.git#src-git base https://github.com/openwrt/openwrt.git#' feeds.conf.default
./scripts/feeds update base packages luci soya
./scripts/feeds install luci-base soya-asn-router luci-app-soya-asn-router
```

The default package workflow expects a prebuilt Rust binary. This avoids
building OpenWrt `rust/host`, which can be slow and brittle in SDK-only flows.
Point `SOYA_ASN_ROUTER_PREBUILT` at a binary built for the router target, for
example `aarch64-unknown-linux-musl`:

```sh
echo 'CONFIG_PACKAGE_soya-asn-router=m' >> .config
echo 'CONFIG_PACKAGE_luci-app-soya-asn-router=m' >> .config
make defconfig
make package/feeds/soya/soya-asn-router/compile V=s \
  SOYA_ASN_ROUTER_PREBUILT=/absolute/path/to/soya-asn-router
make package/feeds/soya/luci-app-soya-asn-router/compile V=s \
  SOYA_ASN_ROUTER_PREBUILT=/absolute/path/to/soya-asn-router
```

For packages embedded into a firmware image, use `=y` instead of `=m` and then
run `make`.

The built package files will be under:

```sh
bin/packages/*/soya/
```

Depending on the OpenWrt branch, the package files will be `.apk` or `.ipk`.

If the OpenWrt Rust toolchain works in your buildroot, you can build from source
instead:

```text
CONFIG_SOYA_ASN_ROUTER_BUILD_FROM_SOURCE=y
```

## GitHub Releases

The repository includes `.github/workflows/openwrt-ipk.yml`. It builds release
artifacts with the official OpenWrt SDK and uploads them either as workflow
artifacts or as GitHub Release assets.

The current matrix builds packages for:

- OpenWrt `24.10.4`, target `mediatek/filogic`, package arch
  `aarch64_cortex-a53`;
- OpenWrt `25.12.2`, target `mediatek/filogic`, package arch
  `aarch64_cortex-a53`.

Before tagging a release, update package versions:

```sh
$EDITOR package/soya-asn-router/Makefile
```

At minimum, bump `PKG_RELEASE` when package contents change. Bump
`PKG_VERSION` when the application version changes.

Create and push a tag:

```sh
git tag -a v0.1.0-r15 -m "soya-asn-router v0.1.0-r15"
git push origin v0.1.0-r15
```

The workflow runs on `v*` tags. If a GitHub Release with the same tag does not
exist, the workflow creates it and uploads assets plus `SHA256SUMS`.

Manual builds are available from GitHub:

1. Open `Actions -> Build OpenWrt Packages`.
2. Run the workflow.
3. Leave `release_tag` empty to keep only workflow artifacts.
4. Set `release_tag` to upload the produced files to that GitHub Release.

To add another OpenWrt target, extend the workflow matrix with:

- `openwrt_release`;
- `openwrt_target`;
- `target_slug`;
- `package_arch`;
- `rust_target`;
- official SDK `sdk_url`;
- SDK `sdk_sha256`.

The SDK URL and SHA256 should be taken from the matching directory under:

```text
https://downloads.openwrt.org/releases/<version>/targets/<target>/<subtarget>/
```

## Package Contents

- `soya-asn-router`: Rust backend managed by procd and exposed through rpcd.
- `luci-app-soya-asn-router`: LuCI UI for ASN sync, route policy, bulk edits,
  custom IPv4 routes, and interface groups.

Runtime state is stored under:

```sh
/etc/soya-asn-router/
```

The default SQLite database path is:

```sh
/etc/soya-asn-router/soya.db
```
