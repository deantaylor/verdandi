-- Run from the Verdandi repo root with Lua 5.3.
package.loaded.tabutil = {}
package.loaded["core/paramset"] = {}
package.loaded.util = {
  string_starts = function(s, prefix) return s:sub(1, #prefix) == prefix end
}

local calls = {}
local function record(label)
  return function(...) calls[#calls + 1] = label .. ":" .. table.concat({...}, ",") end
end

_norns = {
  osc = {},
  show_control = record("show"),
  key = record("key"),
  enc = record("enc")
}
local osc = dofile("lua/core/osc.lua")
osc.event = function(path) calls[#calls + 1] = path end

local function check(path, args, expected)
  calls = {}
  _norns.osc.event(path, args, {"127.0.0.1", "12345"})
  assert(table.concat(calls, " ") == expected, table.concat(calls, " "))
end

check("/remote/key", {1, 1}, "/remote/key show:1,1,1 key:1,1")
check("/remote/key", {1, 0}, "/remote/key show:1,1,0 key:1,0")
check("/remote/key/3", {1}, "/remote/key/3 show:1,3,1 key:3,1")
check("/remote/enc", {2, -3}, "/remote/enc show:2,2,-3 enc:2,-3")

_norns.show_control = nil
check("/remote/key", {2, 1}, "/remote/key key:2,1")
print("OSC control feedback tests passed")
