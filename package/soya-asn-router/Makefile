# SPDX-License-Identifier: MIT

include $(TOPDIR)/rules.mk

PKG_NAME:=soya-asn-router
PKG_VERSION:=0.1.0
PKG_RELEASE:=13

PKG_LICENSE:=MIT
PKG_LICENSE_FILES:=LICENSE
PKG_MAINTAINER:=Soya OpenWrt contributors

PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_VERSION)
PKG_BUILD_PARALLEL:=1

SOYA_ASN_ROUTER_PREBUILT ?=

ifeq ($(strip $(SOYA_ASN_ROUTER_PREBUILT)),)
ifeq ($(CONFIG_SOYA_ASN_ROUTER_BUILD_FROM_SOURCE),y)
  PKG_BUILD_DEPENDS:=rust/host
  RUST_PKG_LOCKED:=1
endif
endif

include $(INCLUDE_DIR)/package.mk

ifeq ($(strip $(SOYA_ASN_ROUTER_PREBUILT)),)
ifeq ($(CONFIG_SOYA_ASN_ROUTER_BUILD_FROM_SOURCE),y)
  RUST_PACKAGE_MK:=$(TOPDIR)/feeds/packages/lang/rust/rust-package.mk
  ifeq ($(wildcard $(RUST_PACKAGE_MK)),)
    RUST_PACKAGE_MK:=$(TOPDIR)/package/feeds/packages/rust/rust-package.mk
  endif
  include $(RUST_PACKAGE_MK)
endif
endif

define Package/soya-asn-router
  SECTION:=utils
  CATEGORY:=Utilities
  TITLE:=Soya ASN Router Rust backend
  DEPENDS:=$(if $(CONFIG_SOYA_ASN_ROUTER_BUILD_FROM_SOURCE),$(RUST_ARCH_DEPENDS)) +curl +ip +jshn +kmod-nft-core +nftables-json +rpcd +ubus +uci
endef

define Package/soya-asn-router/config
  config SOYA_ASN_ROUTER_BUILD_FROM_SOURCE
	bool "Build soya-asn-router Rust backend from source"
	default n
	help
	  Enable this only when the OpenWrt Rust toolchain builds cleanly for the
	  selected SDK/buildroot. The default package workflow uses
	  SOYA_ASN_ROUTER_PREBUILT=/path/to/soya-asn-router to avoid building rust/host.
endef

define Package/soya-asn-router/description
Rust backend for ASN prefix synchronization and LAN-only IPv4 route policy
generation. It stores RIPE announced prefixes in a persistent SQLite database
and exposes status/sync/route commands through rpcd.
endef

define Package/soya-asn-router/conffiles
/etc/config/soya-asn-router
endef

define Build/Prepare
	$(call Build/Prepare/Default)
	$(CP) ./src/Cargo.toml ./src/Cargo.lock ./src/LICENSE $(PKG_BUILD_DIR)/
	$(CP) ./src/src $(PKG_BUILD_DIR)/
endef

ifneq ($(strip $(SOYA_ASN_ROUTER_PREBUILT)),)
define Build/Compile
	$(INSTALL_DIR) $(PKG_INSTALL_DIR)/bin
	$(INSTALL_BIN) $(SOYA_ASN_ROUTER_PREBUILT) $(PKG_INSTALL_DIR)/bin/soya-asn-router
endef
else ifneq ($(CONFIG_SOYA_ASN_ROUTER_BUILD_FROM_SOURCE),y)
define Build/Compile
	$(error Set SOYA_ASN_ROUTER_PREBUILT=/path/to/soya-asn-router or enable CONFIG_SOYA_ASN_ROUTER_BUILD_FROM_SOURCE)
endef
endif

define Package/soya-asn-router/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/soya-asn-router $(1)/usr/bin/soya-asn-router
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) ./files/etc/init.d/soya-asn-router $(1)/etc/init.d/soya-asn-router
	$(INSTALL_DIR) $(1)/etc/config
	$(INSTALL_CONF) ./files/etc/config/soya-asn-router $(1)/etc/config/soya-asn-router
	$(INSTALL_DIR) $(1)/etc/uci-defaults
	$(INSTALL_BIN) ./files/etc/uci-defaults/99-soya-asn-router-migrate $(1)/etc/uci-defaults/99-soya-asn-router-migrate
	$(INSTALL_DIR) $(1)/etc/soya-asn-router
	$(INSTALL_DIR) $(1)/usr/libexec/rpcd
	$(INSTALL_BIN) ./files/usr/libexec/rpcd/soya-asn-router $(1)/usr/libexec/rpcd/soya-asn-router
	$(INSTALL_DIR) $(1)/lib/upgrade/keep.d
	$(INSTALL_DATA) ./files/lib/upgrade/keep.d/soya-asn-router $(1)/lib/upgrade/keep.d/soya-asn-router
endef

ifeq ($(CONFIG_SOYA_ASN_ROUTER_BUILD_FROM_SOURCE),y)
  $(eval $(call RustBinPackage,soya-asn-router))
endif
$(eval $(call BuildPackage,soya-asn-router))
