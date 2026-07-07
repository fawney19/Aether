#!/usr/bin/env node
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const reportPath = process.argv[2] || '/tmp/aether_gateway_pressure_s1_1k.json'
const checker = path.join(__dirname, 'check_gateway_stage_report.js')
const result = spawnSync(process.execPath, [checker, '--stage', 'S1', reportPath], {
  stdio: 'inherit',
})

process.exit(result.status ?? 1)
