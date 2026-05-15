'use strict';
'require view';
'require form';
'require rpc';
'require poll';
'require ui';

var statusBody = null;
var statusSummary = null;
var pollRegistered = false;
var latestStatus = null;
var routeToggleButton = null;
var statusSelectAll = null;
var bulkInterfaceSelect = null;
var bulkSelectedCount = null;
var bulkDeleteButton = null;
var bulkSetInterfaceButton = null;
var selectedAsns = {};
var actionWidgets = {};
var actionInFlight = false;

var callStatus = rpc.declare({
	object: 'soya-asn-router',
	method: 'status',
	expect: { '': {} }
});

var callInterfaces = rpc.declare({
	object: 'soya-asn-router',
	method: 'interfaces',
	expect: { '': { interfaces: [] } }
});

var callSyncMissing = rpc.declare({
	object: 'soya-asn-router',
	method: 'sync_missing',
	expect: { '': {} }
});

var callSyncAll = rpc.declare({
	object: 'soya-asn-router',
	method: 'sync_all',
	expect: { '': {} }
});

var callApplyRoutes = rpc.declare({
	object: 'soya-asn-router',
	method: 'apply_routes',
	expect: { '': {} }
});

var callPreviewRoutes = rpc.declare({
	object: 'soya-asn-router',
	method: 'preview_routes',
	expect: { '': {} }
});

var callPauseRoutes = rpc.declare({
	object: 'soya-asn-router',
	method: 'pause_routes',
	expect: { '': {} }
});

var callResumeRoutes = rpc.declare({
	object: 'soya-asn-router',
	method: 'resume_routes',
	expect: { '': {} }
});

var callDedupeConfig = rpc.declare({
	object: 'soya-asn-router',
	method: 'dedupe_config',
	expect: { '': {} }
});

var callImportUrl = rpc.declare({
	object: 'soya-asn-router',
	method: 'import_url',
	params: [ 'url', 'interface' ],
	expect: { '': {} }
});

var callBulkDelete = rpc.declare({
	object: 'soya-asn-router',
	method: 'bulk_delete',
	params: [ 'asns' ],
	expect: { '': {} }
});

var callBulkSetInterface = rpc.declare({
	object: 'soya-asn-router',
	method: 'bulk_set_interface',
	params: [ 'asns', 'interface' ],
	expect: { '': {} }
});

function stateText(state) {
	switch (state) {
	case 'queued':
		return _('queued');
	case 'syncing':
		return _('syncing');
	case 'synced':
		return _('synced');
	case 'error':
		return _('error');
	default:
		return _('not synced');
	}
}

function policyText(state) {
	switch (state) {
	case 'applied':
		return _('applied');
	case 'generated':
		return _('generated');
	case 'error':
		return _('error');
	case 'paused':
		return _('paused');
	default:
		return _('not applied');
	}
}

function formatValue(value) {
	return value == null || value === '' ? '-' : value;
}

function interfaceLabel(item) {
	var label = item.name || '-';

	if (item.device)
		label += ' (' + item.device + ')';

	if (!item.up)
		label += ' / ' + _('down');

	return label;
}

function normalizeInterfaces(response) {
	var interfaces = (response && response.interfaces) || [];
	var seen = {};
	var result = [];

	interfaces.forEach(function(item) {
		if (!item || !item.name || seen[item.name])
			return;

		seen[item.name] = true;
		result.push(item);
	});

	[ 'lan', 'wan' ].forEach(function(name) {
		if (!seen[name]) {
			seen[name] = true;
			result.push({ name: name, device: null, up: true });
		}
	});

	return result;
}

function addInterfaceValues(option, interfaces) {
	interfaces.forEach(function(item) {
		option.value(item.name, interfaceLabel(item));
	});
}

function selectedAsnList() {
	return Object.keys(selectedAsns).sort();
}

function cleanupSelection(data) {
	var available = {};

	((data && data.asns) || []).forEach(function(item) {
		available[item.asn] = true;
	});

	Object.keys(selectedAsns).forEach(function(asn) {
		if (!available[asn])
			delete selectedAsns[asn];
	});
}

function statusBusy(data) {
	return actionInFlight || !!(data && data.sync && data.sync.running);
}

function setDisabled(node, disabled) {
	var controls;

	if (!node)
		return;

	controls = node.matches && node.matches('button,input,select')
		? [ node ]
		: node.querySelectorAll('button,input,select');

	Array.prototype.forEach.call(controls, function(control) {
		control.disabled = disabled;
	});
}

function registerActionWidget(key, widget) {
	actionWidgets[key] = widget;
	updateActionState(latestStatus);
	return widget;
}

function updateActionState(data) {
	var busy = statusBusy(data);

	[ 'sync_missing', 'sync_all', 'apply_routes', 'preview_routes', 'toggle_routes', 'import_asns' ].forEach(function(key) {
		setDisabled(actionWidgets[key], busy);
	});

	updateBulkControls(data);
}

function updateBulkControls(data) {
	var list, busy, allVisibleSelected, visibleAsns;

	cleanupSelection(data);
	list = selectedAsnList();
	busy = statusBusy(data);
	visibleAsns = ((data && data.asns) || []).map(function(item) {
		return item.asn;
	});
	allVisibleSelected = visibleAsns.length > 0 && visibleAsns.every(function(asn) {
		return !!selectedAsns[asn];
	});

	if (bulkSelectedCount)
		bulkSelectedCount.textContent = _('%d selected').format(list.length);

	if (bulkDeleteButton)
		bulkDeleteButton.disabled = busy || list.length === 0;

	if (bulkSetInterfaceButton)
		bulkSetInterfaceButton.disabled = busy || list.length === 0 || !bulkInterfaceSelect || !bulkInterfaceSelect.value;

	if (bulkInterfaceSelect)
		bulkInterfaceSelect.disabled = busy;

	if (statusSelectAll) {
		statusSelectAll.checked = allVisibleSelected;
		statusSelectAll.indeterminate = !allVisibleSelected && list.length > 0;
		statusSelectAll.disabled = busy || visibleAsns.length === 0;
	}
}

function selectVisibleAsns(data, selected) {
	((data && data.asns) || []).forEach(function(item) {
		if (selected)
			selectedAsns[item.asn] = true;
		else
			delete selectedAsns[item.asn];
	});

	if (statusBody)
		statusBody.replaceChildren.apply(statusBody, renderRows(data || { asns: [] }));

	updateBulkControls(data);
}

function renderRows(data) {
	var asns = data.asns || [];

	if (asns.length === 0) {
		return [
			E('tr', { 'class': 'tr placeholder' }, [
				E('td', { 'class': 'td', 'colspan': 10 }, [
					E('em', {}, [ _('No ASNs configured.') ])
				])
			])
		];
	}

	return asns.map(function(item) {
		var checkboxAttrs = { 'type': 'checkbox' };
		var checkbox;

		if (selectedAsns[item.asn])
			checkboxAttrs.checked = 'checked';

		if (statusBusy(data))
			checkboxAttrs.disabled = 'disabled';

		checkbox = E('input', checkboxAttrs);

		checkbox.addEventListener('change', function() {
			if (checkbox.checked)
				selectedAsns[item.asn] = true;
			else
				delete selectedAsns[item.asn];

			updateBulkControls(latestStatus);
		});

		return E('tr', { 'class': 'tr' }, [
			E('td', { 'class': 'td' }, [ checkbox ]),
			E('td', { 'class': 'td' }, [ item.asn ]),
			E('td', { 'class': 'td' }, [ formatValue(item.provider_name) ]),
			E('td', { 'class': 'td' }, [ item.enabled ? _('yes') : _('no') ]),
			E('td', { 'class': 'td' }, [ formatValue(item.target_interface) ]),
			E('td', { 'class': 'td' }, [ stateText(item.state) ]),
			E('td', { 'class': 'td' }, [ String(item.ipv4_count || 0) ]),
			E('td', { 'class': 'td' }, [ String(item.ipv6_count || 0) ]),
			E('td', { 'class': 'td' }, [ formatValue(item.last_synced_at) ]),
			E('td', { 'class': 'td' }, [ formatValue(item.last_error) ])
		]);
	});
}

function renderSummary(data) {
	var sync = data.sync || {};
	var policy = data.policy || {};
	var periodic = data.periodic_sync || {};
	var syncState = sync.running
		? _('Synchronization is running (%s): %d/%d, failed: %d%s.').format(
			sync.mode || _('unknown'),
			sync.completed || 0,
			sync.total || 0,
			sync.failed || 0,
			sync.current_asn ? ', ' + sync.current_asn : ''
		)
		: _('Synchronization is idle.');
	var policyState = _('Policy: %s, IPv4 prefixes: %d, interfaces: %d.').format(
		policy.enabled === false ? _('paused') : policyText(policy.state),
		policy.ipv4_prefix_count || 0,
		policy.interface_count || 0
	);
	var periodicState = periodic.enabled
		? _('Periodic sync: every %d minute(s), mode: %s.').format(
			periodic.interval_minutes || 0,
			periodic.mode || _('unknown')
		)
		: _('Periodic sync: disabled.');

	if (policy.last_error)
		policyState += ' ' + _('Error: %s').format(policy.last_error);

	return [
		E('span', {}, [ syncState ]),
		E('br'),
		E('span', {}, [ periodicState ]),
		E('br'),
		E('span', {}, [ policyState ]),
		E('br'),
		E('span', {}, [ _('Database: %s').format(data.db_path || '-') ])
	];
}

function routeToggleTitle(data) {
	var policy = data && data.policy ? data.policy : {};

	return policy.enabled === false
		? _('Start route policies')
		: _('Pause route policies');
}

function updateRouteToggleButton(data) {
	var buttons, paused, title;

	if (!routeToggleButton)
		return;

	title = routeToggleTitle(data);
	paused = data && data.policy && data.policy.enabled === false;
	buttons = routeToggleButton.matches && routeToggleButton.matches('button,input')
		? [ routeToggleButton ]
		: routeToggleButton.querySelectorAll('button,input');

	Array.prototype.forEach.call(buttons, function(button) {
		if (button.tagName === 'INPUT')
			button.value = title;
		else
			button.textContent = title;

		button.title = title;
		button.classList.remove('cbi-button-remove', 'cbi-button-apply');
		button.classList.add(paused ? 'cbi-button-apply' : 'cbi-button-remove');
	});
}

function applyStatus(data) {
	if (!data || !statusBody || !statusSummary)
		return;

	latestStatus = data;
	cleanupSelection(data);
	statusBody.replaceChildren.apply(statusBody, renderRows(data));
	statusSummary.replaceChildren.apply(statusSummary, renderSummary(data));
	updateRouteToggleButton(data);
	updateActionState(data);
}

function updateStatus() {
	return L.resolveDefault(callStatus(), null).then(function(data) {
		applyStatus(data);
	});
}

function notifyError(error) {
	ui.addNotification(null, E('p', {}, [
		error && error.message ? error.message : String(error)
	]), 'error');
}

function notifyInfo(message) {
	ui.addNotification(null, E('p', {}, [ message ]), 'info');
}

function runAction(call) {
	actionInFlight = true;
	updateActionState(latestStatus);

	return call().then(function(result) {
		if (result && result.policy && result.asns)
			applyStatus(result);
		else
			return updateStatus();
	}).catch(notifyError).then(function() {
		actionInFlight = false;
		updateActionState(latestStatus);
		window.setTimeout(updateStatus, 600);
		window.setTimeout(updateStatus, 2000);
	});
}

function cleanupConfig() {
	return callDedupeConfig().then(function(result) {
		var removed = (result.removed_duplicates || 0) + (result.removed_invalid || 0);

		if (removed > 0)
			notifyInfo(_('Removed %d duplicate or invalid ASN row(s).').format(removed));

		return result;
	});
}

function syncAfterConfigSave() {
	var removed = 0;

	return cleanupConfig().then(function(result) {
		removed = (result.removed_duplicates || 0) + (result.removed_invalid || 0);
		return runAction(callSyncMissing);
	}).then(function() {
		if (removed > 0)
			window.setTimeout(function() {
				window.location.reload();
			}, 700);
	}).catch(notifyError);
}

function importFromUrl(url, iface) {
	url = (url || '').trim();
	iface = (iface || '').trim();

	if (!url) {
		return Promise.reject(new Error(_('Import URL is required.')));
	}

	if (!iface) {
		return Promise.reject(new Error(_('Target interface is required.')));
	}

	return callImportUrl(url, iface).then(function(result) {
		var skipped = (result.skipped_existing || 0) + (result.skipped_duplicate || 0);
		var message = _('Imported %d ASN(s) to %s. Skipped %d existing or duplicate item(s).').format(
			result.imported || 0,
			result.interface || iface,
			skipped
		);

		if (result.invalid)
			message += ' ' + _('Ignored invalid item(s): %d.').format(result.invalid);

		if (result.deduplicated)
			message += ' ' + _('Cleaned existing duplicate row(s): %d.').format(result.deduplicated);

		notifyInfo(message);
		return updateStatus();
	}).then(function() {
		window.setTimeout(function() {
			window.location.reload();
		}, 900);
	});
}

function reloadSoon() {
	window.setTimeout(function() {
		window.location.reload();
	}, 900);
}

function bulkDeleteSelected() {
	var list = selectedAsnList();

	if (list.length === 0)
		return Promise.resolve();

	if (!window.confirm(_('Delete selected ASN row(s)?')))
		return Promise.resolve();

	actionInFlight = true;
	updateActionState(latestStatus);

	return callBulkDelete(list.join(' ')).then(function(result) {
		selectedAsns = {};
		notifyInfo(_('Deleted %d ASN row(s). Missing: %d.').format(
			result.deleted || 0,
			result.missing || 0
		));
		return updateStatus();
	}).then(reloadSoon).catch(notifyError).then(function() {
		actionInFlight = false;
		updateActionState(latestStatus);
	});
}

function bulkSetInterfaceSelected() {
	var list = selectedAsnList();
	var iface = bulkInterfaceSelect ? bulkInterfaceSelect.value : '';

	if (list.length === 0 || !iface)
		return Promise.resolve();

	actionInFlight = true;
	updateActionState(latestStatus);

	return callBulkSetInterface(list.join(' '), iface).then(function(result) {
		notifyInfo(_('Updated %d ASN row(s) to %s. Missing: %d.').format(
			result.updated || 0,
			result.interface || iface,
			result.missing || 0
		));
		return updateStatus();
	}).then(reloadSoon).catch(notifyError).then(function() {
		actionInFlight = false;
		updateActionState(latestStatus);
	});
}

function renderBulkControls(interfaces) {
	bulkSelectedCount = E('span', { 'class': 'cbi-value-description' }, [ _('0 selected') ]);
	bulkInterfaceSelect = E('select', { 'class': 'cbi-input-select' },
		interfaces.map(function(item) {
			return E('option', { 'value': item.name }, [ interfaceLabel(item) ]);
		})
	);
	bulkSetInterfaceButton = E('button', {
		'class': 'btn cbi-button cbi-button-action'
	}, [ _('Set interface') ]);
	bulkDeleteButton = E('button', {
		'class': 'btn cbi-button cbi-button-remove'
	}, [ _('Delete selected') ]);

	bulkSetInterfaceButton.addEventListener('click', function(ev) {
		ev.preventDefault();
		bulkSetInterfaceSelected();
	});

	bulkDeleteButton.addEventListener('click', function(ev) {
		ev.preventDefault();
		bulkDeleteSelected();
	});

	bulkInterfaceSelect.addEventListener('change', function() {
		updateBulkControls(latestStatus);
	});

	return E('div', { 'class': 'cbi-value' }, [
		E('label', { 'class': 'cbi-value-title' }, [ _('Bulk actions') ]),
		E('div', { 'class': 'cbi-value-field' }, [
			bulkSelectedCount,
			' ',
			bulkInterfaceSelect,
			' ',
			bulkSetInterfaceButton,
			' ',
			bulkDeleteButton
		])
	]);
}

function showImportModal(interfaces) {
	var defaultInterface = latestStatus && latestStatus.default_target_interface
		? latestStatus.default_target_interface
		: 'wan';
	var urlInput = E('input', {
		'class': 'cbi-input-text',
		'type': 'url',
		'placeholder': 'https://example.com/asns.txt',
		'style': 'width: 100%'
	});
	var interfaceSelect = E('select', { 'class': 'cbi-input-select' },
		interfaces.map(function(item) {
			var attrs = { 'value': item.name };

			if (item.name === defaultInterface)
				attrs.selected = 'selected';

			return E('option', attrs, [ interfaceLabel(item) ]);
		})
	);
	var importButton = E('button', {
		'class': 'btn cbi-button cbi-button-action'
	}, [ _('Import') ]);
	var cancelButton = E('button', {
		'class': 'btn cbi-button cbi-button-neutral'
	}, [ _('Cancel') ]);

	cancelButton.addEventListener('click', function(ev) {
		ev.preventDefault();
		ui.hideModal();
	});

	importButton.addEventListener('click', function(ev) {
		ev.preventDefault();
		importButton.disabled = true;

		importFromUrl(urlInput.value, interfaceSelect.value).then(function() {
			ui.hideModal();
		}).catch(function(error) {
			importButton.disabled = false;
			notifyError(error);
		});
	});

	ui.showModal(_('Import ASNs from URL'), [
		E('div', { 'class': 'cbi-section' }, [
			E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' }, [ _('URL') ]),
				E('div', { 'class': 'cbi-value-field' }, [ urlInput ])
			]),
			E('div', { 'class': 'cbi-value' }, [
				E('label', { 'class': 'cbi-value-title' }, [ _('Target interface') ]),
				E('div', { 'class': 'cbi-value-field' }, [ interfaceSelect ])
			])
		]),
		E('div', { 'class': 'right' }, [
			cancelButton,
			' ',
			importButton
		])
	]);

	window.setTimeout(function() {
		urlInput.focus();
	}, 0);

	return Promise.resolve();
}

function renderPreviewRows(preview) {
	var groups = preview.groups || [];

	if (groups.length === 0) {
		return [
			E('tr', { 'class': 'tr placeholder' }, [
				E('td', { 'class': 'td', 'colspan': 7 }, [
					E('em', {}, [ _('No IPv4 prefixes are ready to apply.') ])
				])
			])
		];
	}

	return groups.map(function(group) {
		return E('tr', { 'class': 'tr' }, [
			E('td', { 'class': 'td' }, [ group.interface || '-' ]),
			E('td', { 'class': 'td' }, [ group.device || '-' ]),
			E('td', { 'class': 'td' }, [ String(group.asn_count || 0) ]),
			E('td', { 'class': 'td' }, [ String(group.ipv4_prefix_count || 0) ]),
			E('td', { 'class': 'td' }, [ '0x' + (group.mark || 0).toString(16) ]),
			E('td', { 'class': 'td' }, [ String(group.table_id || 0) ]),
			E('td', { 'class': 'td' }, [ formatValue(group.default_route_nexthop) ])
		]);
	});
}

function showRoutePreview() {
	actionInFlight = true;
	updateActionState(latestStatus);

	return callPreviewRoutes().then(function(preview) {
		var applyButton = E('button', {
			'class': 'btn cbi-button cbi-button-apply'
		}, [ _('Apply') ]);
		var cancelButton = E('button', {
			'class': 'btn cbi-button cbi-button-neutral'
		}, [ _('Cancel') ]);

		cancelButton.addEventListener('click', function(ev) {
			ev.preventDefault();
			ui.hideModal();
		});

		applyButton.addEventListener('click', function(ev) {
			ev.preventDefault();
			applyButton.disabled = true;
			runAction(callApplyRoutes).then(function() {
				ui.hideModal();
			});
		});

		ui.showModal(_('Route policy preview'), [
			E('p', {}, [
				_('IPv4 prefixes: %d, target interfaces: %d.').format(
					preview.ipv4_prefix_count || 0,
					preview.interface_count || 0
				)
			]),
			E('table', { 'class': 'table' }, [
				E('thead', {}, [
					E('tr', { 'class': 'tr table-titles' }, [
						E('th', { 'class': 'th' }, [ _('Interface') ]),
						E('th', { 'class': 'th' }, [ _('Device') ]),
						E('th', { 'class': 'th' }, [ _('ASNs') ]),
						E('th', { 'class': 'th' }, [ _('IPv4') ]),
						E('th', { 'class': 'th' }, [ _('Mark') ]),
						E('th', { 'class': 'th' }, [ _('Table') ]),
						E('th', { 'class': 'th' }, [ _('Nexthop') ])
					])
				]),
				E('tbody', {}, renderPreviewRows(preview))
			]),
			E('div', { 'class': 'right' }, [
				cancelButton,
				' ',
				applyButton
			])
		]);
	}).catch(notifyError).then(function() {
		actionInFlight = false;
		updateActionState(latestStatus);
	});
}

function toggleRoutes() {
	var policy = latestStatus && latestStatus.policy ? latestStatus.policy : {};
	var call = policy.enabled === false ? callResumeRoutes : callPauseRoutes;

	return runAction(call).then(function() {
		updateRouteToggleButton(latestStatus);
		window.setTimeout(updateStatus, 1000);
	});
}

function validateAsn(_sectionId, value) {
	if (value == null || value === '')
		return true;

	return /^(AS)?[0-9]{1,10}$/i.test(value)
		? true
		: _('Use ASN format like AS15169 or 15169.');
}

function validatePositiveInteger(_sectionId, value) {
	if (value == null || value === '')
		return true;

	return /^[0-9]+$/.test(value) && Number(value) > 0
		? true
		: _('Use a positive integer.');
}

return view.extend({
	load: function() {
		return Promise.all([
			L.resolveDefault(callInterfaces(), { interfaces: [] }),
			L.resolveDefault(callStatus(), null)
		]);
	},

	render: function(data) {
		var m, s, o;
		var interfaceResponse = data[0];
		latestStatus = data[1];
		var interfaces = normalizeInterfaces(interfaceResponse);

		m = new form.Map('soya-asn-router', _('Soya ASN Router'));

		s = m.section(form.NamedSection, 'main', 'service', _('Service settings'));
		s.anonymous = true;

		o = s.option(form.Flag, 'enabled', _('Enable backend daemon'));
		o.default = '0';
		o.rmempty = false;

		o = s.option(form.ListValue, 'lan_interface', _('LAN source interface'));
		addInterfaceValues(o, interfaces);
		o.default = 'lan';
		o.rmempty = false;

		o = s.option(form.ListValue, 'default_interface', _('Default target interface'));
		addInterfaceValues(o, interfaces);
		o.default = 'wan';
		o.rmempty = false;

		o = s.option(form.Flag, 'auto_apply_routes', _('Apply route policies after synchronization'));
		o.default = '1';
		o.rmempty = false;

		o = s.option(form.Flag, 'periodic_sync_enabled', _('Periodic synchronization'));
		o.default = '0';
		o.rmempty = false;

		o = s.option(form.ListValue, 'periodic_sync_mode', _('Periodic synchronization mode'));
		o.value('all', _('All configured ASNs'));
		o.value('missing', _('Missing only'));
		o.default = 'all';
		o.depends('periodic_sync_enabled', '1');
		o.rmempty = false;

		o = s.option(form.Value, 'periodic_sync_interval_minutes', _('Periodic synchronization interval'));
		o.default = '1440';
		o.placeholder = '1440';
		o.depends('periodic_sync_enabled', '1');
		o.rmempty = false;
		o.validate = validatePositiveInteger;

		o = s.option(form.Flag, 'proxy_enabled', _('Use proxy'));
		o.default = '0';
		o.rmempty = false;

		o = s.option(form.ListValue, 'proxy_type', _('Proxy type'));
		o.value('http', _('HTTP'));
		o.value('socks5', _('SOCKS5'));
		o.default = 'http';
		o.depends('proxy_enabled', '1');
		o.rmempty = false;

		o = s.option(form.Value, 'proxy_url', _('Proxy URL'));
		o.placeholder = '127.0.0.1:1080';
		o.depends('proxy_enabled', '1');
		o.rmempty = true;

		o = s.option(form.Value, 'db_path', _('SQLite database path'));
		o.default = '/etc/soya-asn-router/soya.db';
		o.placeholder = '/etc/soya-asn-router/soya.db';
		o.rmempty = false;

		o = s.option(form.Button, '_import_asns', _('Import ASNs from URL'));
		o.inputstyle = 'action';
		o.renderWidget = function(sectionId, optionIndex, cfgvalue) {
			return registerActionWidget('import_asns', form.Button.prototype.renderWidget.call(this, sectionId, optionIndex, cfgvalue));
		};
		o.onclick = function() {
			return showImportModal(interfaces);
		};

		o = s.option(form.Button, '_sync_missing', _('Synchronize missing'));
		o.inputstyle = 'action';
		o.renderWidget = function(sectionId, optionIndex, cfgvalue) {
			return registerActionWidget('sync_missing', form.Button.prototype.renderWidget.call(this, sectionId, optionIndex, cfgvalue));
		};
		o.onclick = function() {
			return runAction(callSyncMissing);
		};

		o = s.option(form.Button, '_sync_all', _('Synchronize all'));
		o.inputstyle = 'apply';
		o.renderWidget = function(sectionId, optionIndex, cfgvalue) {
			return registerActionWidget('sync_all', form.Button.prototype.renderWidget.call(this, sectionId, optionIndex, cfgvalue));
		};
		o.onclick = function() {
			return runAction(callSyncAll);
		};

		o = s.option(form.Button, '_preview_routes', _('Preview route policies'));
		o.inputstyle = 'action';
		o.renderWidget = function(sectionId, optionIndex, cfgvalue) {
			return registerActionWidget('preview_routes', form.Button.prototype.renderWidget.call(this, sectionId, optionIndex, cfgvalue));
		};
		o.onclick = showRoutePreview;

		o = s.option(form.Button, '_apply_routes', _('Apply route policies'));
		o.inputstyle = 'reload';
		o.renderWidget = function(sectionId, optionIndex, cfgvalue) {
			return registerActionWidget('apply_routes', form.Button.prototype.renderWidget.call(this, sectionId, optionIndex, cfgvalue));
		};
		o.onclick = function() {
			return runAction(callApplyRoutes);
		};

		o = s.option(form.Button, '_toggle_routes', routeToggleTitle(latestStatus));
		o.inputstyle = 'remove';
		o.inputtitle = routeToggleTitle(latestStatus);
		o.renderWidget = function(sectionId, optionIndex, cfgvalue) {
			routeToggleButton = form.Button.prototype.renderWidget.call(this, sectionId, optionIndex, cfgvalue);
			registerActionWidget('toggle_routes', routeToggleButton);
			updateRouteToggleButton(latestStatus);
			return routeToggleButton;
		};
		o.onclick = toggleRoutes;

		s = m.section(form.GridSection, 'asn', _('ASNs'));
		s.anonymous = true;
		s.addremove = true;
		s.sortable = true;

		o = s.option(form.Flag, 'enabled', _('Enabled'));
		o.default = '1';
		o.rmempty = false;

		o = s.option(form.Value, 'asn', _('ASN'));
		o.placeholder = 'AS15169';
		o.rmempty = false;
		o.validate = validateAsn;

		o = s.option(form.ListValue, 'interface', _('Target interface'));
		addInterfaceValues(o, interfaces);
		o.default = 'wan';
		o.rmempty = false;

		return m.render().then(function(mapNode) {
			statusSummary = E('div', { 'class': 'cbi-value-description' }, [
				_('Collecting data ...')
			]);

			statusBody = E('tbody', {}, [
				E('tr', { 'class': 'tr placeholder' }, [
					E('td', { 'class': 'td', 'colspan': 10 }, [
						E('em', {}, [ _('Collecting data ...') ])
					])
				])
			]);

			statusSelectAll = E('input', { 'type': 'checkbox' });
			statusSelectAll.addEventListener('change', function() {
				selectVisibleAsns(latestStatus, statusSelectAll.checked);
			});

			var statusNode = E('div', { 'class': 'cbi-section' }, [
				E('h3', {}, [ _('Synchronization status') ]),
				statusSummary,
				renderBulkControls(interfaces),
				E('table', { 'class': 'table' }, [
					E('thead', {}, [
						E('tr', { 'class': 'tr table-titles' }, [
							E('th', { 'class': 'th' }, [ statusSelectAll ]),
							E('th', { 'class': 'th' }, [ _('ASN') ]),
							E('th', { 'class': 'th' }, [ _('Provider') ]),
							E('th', { 'class': 'th' }, [ _('Enabled') ]),
							E('th', { 'class': 'th' }, [ _('Interface') ]),
							E('th', { 'class': 'th' }, [ _('State') ]),
							E('th', { 'class': 'th' }, [ _('IPv4') ]),
							E('th', { 'class': 'th' }, [ _('IPv6') ]),
							E('th', { 'class': 'th' }, [ _('Last synchronized') ]),
							E('th', { 'class': 'th' }, [ _('Error') ])
						])
					]),
					statusBody
				])
			]);

			if (!pollRegistered) {
				poll.add(updateStatus, 2);
				pollRegistered = true;
			}

			updateStatus();
			return E('div', {}, [ mapNode, statusNode ]);
		});
	},

	handleSave: function(ev) {
		return this.super('handleSave', [ ev ]).then(function() {
			return syncAfterConfigSave();
		});
	},

	handleSaveApply: function(ev, mode) {
		return this.super('handleSave', [ ev ]).then(function() {
			return ui.changes.apply(mode == '0');
		}).then(function() {
			return syncAfterConfigSave();
		});
	}
});
