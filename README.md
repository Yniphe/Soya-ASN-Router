# Soya ASN Router OpenWrt feed

External OpenWrt feed with:

- `soya-asn-router`: a Rust backend managed by procd and rpcd.
- `luci-app-soya-asn-router`: a LuCI page for ASN-based route policy control.

Soya ASN Router synchronizes RIPE announced prefixes for configured ASNs,
stores them in SQLite, and can route LAN-originated IPv4 traffic to a selected
OpenWrt interface per ASN. The default database path is persistent across
reboot:

```sh
/etc/soya-asn-router/soya.db
```

The package also installs a sysupgrade keep entry for `/etc/soya-asn-router/`.

For IPv4 traffic originated from LAN, the backend can generate and apply route
policies from the synchronized ASN prefixes. The generated files are:

```sh
/etc/soya-asn-router/routes.nft
/etc/soya-asn-router/routes.sh
```

The route policy is grouped by target interface: one nft set and one fwmark
rule/table are generated per selected interface.
When the route policy is enabled, the backend reapplies it on daemon startup,
so it survives router reboots.

The procd service logs startup errors through stdout/stderr redirection, so
messages can be checked with:

```sh
logread -f | grep soya-asn-router
```

## Use with an OpenWrt build tree or SDK

From the OpenWrt tree, add this directory as a linked feed:

```sh
echo "src-link soya /absolute/path/to/soya-openwrt" >> feeds.conf.default
./scripts/feeds update packages luci soya
./scripts/feeds install soya-asn-router luci-app-soya-asn-router
```

The default package workflow expects a prebuilt Rust binary. This avoids
building OpenWrt `rust/host`, which can be slow and brittle in SDK-only flows.
Point `SOYA_ASN_ROUTER_PREBUILT` at an `aarch64-unknown-linux-musl` binary when
building the backend package:

```sh
make package/feeds/soya/soya-asn-router/compile V=s \
  SOYA_ASN_ROUTER_PREBUILT=/absolute/path/to/soya-asn-router
make package/feeds/soya/luci-app-soya-asn-router/compile V=s
```

If the OpenWrt Rust toolchain works in your buildroot, enable
`CONFIG_SOYA_ASN_ROUTER_BUILD_FROM_SOURCE=y` to build from source instead.

## Build quickstart

Use the SDK or buildroot for the same OpenWrt release and target as your
router. You can check the target on a running router with:

```sh
ubus call system board
```

Look for `release.target`, for example `x86/64` or `mediatek/filogic`, then
download the matching SDK from the OpenWrt downloads page.

Inside the extracted SDK/buildroot:

```sh
echo "src-link soya /Users/ivanchikishev/air/soya-openwrt" >> feeds.conf.default
./scripts/feeds update -a
./scripts/feeds install luci-base soya-asn-router luci-app-soya-asn-router
```

Build as standalone packages with a prebuilt backend binary:

```sh
echo 'CONFIG_PACKAGE_soya-asn-router=m' >> .config
echo 'CONFIG_PACKAGE_luci-app-soya-asn-router=m' >> .config
make defconfig
make package/feeds/soya/soya-asn-router/compile V=s \
  SOYA_ASN_ROUTER_PREBUILT=/absolute/path/to/soya-asn-router
make package/feeds/soya/luci-app-soya-asn-router/compile V=s
```

For packages embedded into a firmware image, use `=y` instead of `=m` and then
run `make`.

The built package files will be under:

```sh
bin/packages/*/soya/
```

Depending on the OpenWrt branch, the files will be `.apk` or `.ipk`.

## GitHub Actions releases

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
git tag -a v0.1.0-r4 -m "soya-asn-router v0.1.0-r4"
git push origin v0.1.0-r4
```

The workflow runs on `v*` tags. If a GitHub Release with the same tag does not
exist, the workflow creates it and uploads assets like:

```sh
openwrt-24.10.4-mediatek-filogic-soya-asn-router_0.1.0-r4_aarch64_cortex-a53.ipk
openwrt-24.10.4-mediatek-filogic-luci-app-soya-asn-router_*.ipk
openwrt-25.12.2-mediatek-filogic-soya-asn-router_0.1.0-r4_aarch64_cortex-a53.ipk
openwrt-25.12.2-mediatek-filogic-luci-app-soya-asn-router_*.ipk
SHA256SUMS
```

Use the package set matching the router release and target. Check a router with:

```sh
ubus call system board
```

Look at `release.version` and `release.target`. For example, a router reporting
OpenWrt `24.10.4` and `mediatek/filogic` should use only the
`openwrt-24.10.4-mediatek-filogic-*` assets.

Manual builds are available from GitHub:

1. Open `Actions -> Build OpenWrt IPK`.
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

```sh
https://downloads.openwrt.org/releases/<version>/targets/<target>/<subtarget>/
```

Install release assets on a router with:

```sh
scp openwrt-24.10.4-mediatek-filogic-*.ipk root@192.168.1.1:/tmp/
ssh root@192.168.1.1
opkg install \
  /tmp/openwrt-24.10.4-mediatek-filogic-soya-asn-router_*.ipk \
  /tmp/openwrt-24.10.4-mediatek-filogic-luci-app-soya-asn-router_*.ipk
/etc/init.d/rpcd reload
/etc/init.d/uhttpd reload
/etc/init.d/soya-asn-router restart
```

## Runtime

After installing both packages on the router:

```sh
/etc/init.d/soya-asn-router enable
/etc/init.d/soya-asn-router start
/etc/init.d/rpcd reload
/etc/init.d/uhttpd reload
```

In LuCI, open `Services -> Soya ASN Router`.

The LuCI page supports:

- adding ASNs in `AS15169` or `15169` format;
- selecting a target interface for each ASN;
- displaying the ASN holder name reported by RIPEstat `as-overview`;
- optional HTTP or SOCKS5 proxy configuration;
- synchronizing only ASNs missing from the SQLite database;
- forcing synchronization of all configured ASNs;
- applying LAN-only IPv4 route policies from stored prefixes;
- pausing and resuming route policy application;
- polling per-ASN status without manually refreshing the page;
- switching route policy application between Pause and Start from the service
  settings.

Manual backend commands:

```sh
soya-asn-router status
soya-asn-router interfaces
soya-asn-router sync-missing
soya-asn-router sync-all
soya-asn-router generate-routes
soya-asn-router apply-routes
soya-asn-router pause-routes
soya-asn-router resume-routes
```
