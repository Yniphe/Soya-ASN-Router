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

function renderRows(data) {
	var asns = data.asns || [];

	if (asns.length === 0) {
		return [
			E('tr', { 'class': 'tr placeholder' }, [
				E('td', { 'class': 'td', 'colspan': 9 }, [
					E('em', {}, [ _('No ASNs configured.') ])
				])
			])
		];
	}

	return asns.map(function(item) {
		return E('tr', { 'class': 'tr' }, [
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
	var syncState = sync.running
		? _('Synchronization is running (%s).').format(sync.mode || _('unknown'))
		: _('Synchronization is idle.');
	var policyState = _('Policy: %s, IPv4 prefixes: %d, interfaces: %d.').format(
		policy.enabled === false ? _('paused') : policyText(policy.state),
		policy.ipv4_prefix_count || 0,
		policy.interface_count || 0
	);

	if (policy.last_error)
		policyState += ' ' + _('Error: %s').format(policy.last_error);

	return [
		E('span', {}, [ syncState ]),
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

function updateStatus() {
	return L.resolveDefault(callStatus(), null).then(function(data) {
		if (!data || !statusBody || !statusSummary)
			return;

		latestStatus = data;
		statusBody.replaceChildren.apply(statusBody, renderRows(data));
		statusSummary.replaceChildren.apply(statusSummary, renderSummary(data));
		updateRouteToggleButton(data);
	});
}

function notifyError(error) {
	ui.addNotification(null, E('p', {}, [
		error && error.message ? error.message : String(error)
	]), 'error');
}

function runAction(call) {
	return call().then(function() {
		return updateStatus();
	}).catch(notifyError);
}

function toggleRoutes() {
	var policy = latestStatus && latestStatus.policy ? latestStatus.policy : {};
	var call = policy.enabled === false ? callResumeRoutes : callPauseRoutes;

	return runAction(call).then(function() {
		updateRouteToggleButton(latestStatus);
	});
}

function validateAsn(_sectionId, value) {
	if (value == null || value === '')
		return true;

	return /^(AS)?[0-9]{1,10}$/i.test(value)
		? true
		: _('Use ASN format like AS15169 or 15169.');
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

		o = s.option(form.Button, '_sync_missing', _('Synchronize missing'));
		o.inputstyle = 'action';
		o.onclick = function() {
			return runAction(callSyncMissing);
		};

		o = s.option(form.Button, '_sync_all', _('Synchronize all'));
		o.inputstyle = 'apply';
		o.onclick = function() {
			return runAction(callSyncAll);
		};

		o = s.option(form.Button, '_apply_routes', _('Apply route policies'));
		o.inputstyle = 'reload';
		o.onclick = function() {
			return runAction(callApplyRoutes);
		};

		o = s.option(form.Button, '_toggle_routes', routeToggleTitle(latestStatus));
		o.inputstyle = 'remove';
		o.inputtitle = routeToggleTitle(latestStatus);
		o.renderWidget = function(sectionId, optionIndex, cfgvalue) {
			routeToggleButton = form.Button.prototype.renderWidget.call(this, sectionId, optionIndex, cfgvalue);
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
					E('td', { 'class': 'td', 'colspan': 9 }, [
						E('em', {}, [ _('Collecting data ...') ])
					])
				])
			]);

			var statusNode = E('div', { 'class': 'cbi-section' }, [
				E('h3', {}, [ _('Synchronization status') ]),
				statusSummary,
				E('table', { 'class': 'table' }, [
					E('thead', {}, [
						E('tr', { 'class': 'tr table-titles' }, [
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
			return runAction(callSyncMissing);
		});
	},

	handleSaveApply: function(ev, mode) {
		return this.super('handleSave', [ ev ]).then(function() {
			return ui.changes.apply(mode == '0');
		}).then(function() {
			return runAction(callSyncMissing);
		});
	}
});
