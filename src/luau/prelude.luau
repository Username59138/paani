-- Runs once when the host starts. Receives the Rust natives as its only argument
-- and returns the marker and Luau-implemented methods the host needs.
local native = ...

-- Unique yield tag. A thread that yields (MARK, seconds) is put to sleep by the
-- scheduler; a yield without it is a script's own coroutine and is not ours.
local MARK = {}
-- Yielded by Signal:Wait. The thread stays suspended until the signal fires
-- and its handler resumes it; the scheduler does nothing with it.
local SUSPEND = {}

function wait(seconds)
	return coroutine.yield(MARK, seconds)
end

-- A script's own coroutine that calls wait() yields MARK to whoever resumed
-- it. Hand it to the scheduler instead, so it wakes up later on its own like
-- in Roblox, and the caller sees an ordinary yield.
local rawResume = coroutine.resume

coroutine.resume = function(co, ...)
	local results = table.pack(rawResume(co, ...))
	if results[1] and coroutine.status(co) == "suspended" then
		if results[2] == MARK then
			native.sleep(co, results[3])
			return true
		elseif results[2] == SUSPEND then
			return true
		end
	end
	return table.unpack(results, 1, results.n)
end

coroutine.wrap = function(f)
	local co = coroutine.create(f)
	return function(...)
		local results = table.pack(coroutine.resume(co, ...))
		if not results[1] then
			error(results[2], 2)
		end
		return table.unpack(results, 2, results.n)
	end
end

task = {
	wait = function(seconds)
		local elapsed = coroutine.yield(MARK, seconds)
		return elapsed
	end,
	spawn = native.spawn,
	defer = native.defer,
	delay = native.delay,
	cancel = native.cancel,
}

spawn = native.spawn
delay = native.delay

function time()
	return native.clock()
end

elapsedTime = time

function tick()
	return native.unixClock()
end

function require(moduleScript)
	local cached, value = native.requireCached(moduleScript)
	if cached then
		return value
	end
	local results = table.pack(native.requireChunk(moduleScript)())
	if results.n ~= 1 then
		error("Module code must return exactly one value")
	end
	native.requireStore(moduleScript, results[1])
	return results[1]
end

-- RBXScriptSignal and RBXScriptConnection. Rust decides which members are
-- events and fires them by key (see events.rs).

local Connection = {}
Connection.__index = Connection
Connection.__type = "RBXScriptConnection"

function Connection:Disconnect()
	if not self.Connected then
		return
	end
	self.Connected = false
	local handlers = self._signal._handlers
	local index = table.find(handlers, self)
	if index then
		table.remove(handlers, index)
	end
end

local Signal = {}
Signal.__index = Signal
Signal.__type = "RBXScriptSignal"

function Signal:Connect(handler)
	if type(handler) ~= "function" then
		error("Attempt to connect failed: Passed value is not a function", 2)
	end
	local connection = setmetatable({ Connected = true, _handler = handler, _signal = self }, Connection)
	table.insert(self._handlers, connection)
	return connection
end

function Signal:Once(handler)
	local connection
	connection = self:Connect(function(...)
		connection:Disconnect()
		handler(...)
	end)
	return connection
end

function Signal:Wait()
	local thread = coroutine.running()
	self:Once(function(...)
		task.spawn(thread, ...)
	end)
	return coroutine.yield(SUSPEND)
end

local signalsByKey = {}

local signals = {}

function signals.get(key)
	local signal = signalsByKey[key]
	if signal == nil then
		signal = setmetatable({ _handlers = {} }, Signal)
		signalsByKey[key] = signal
	end
	return signal
end

-- Each handler runs on its own thread, so one that waits or fails doesn't stop the others.
function signals.fire(key, ...)
	local signal = signalsByKey[key]
	if signal == nil then
		return
	end
	for _, connection in table.clone(signal._handlers) do
		if connection.Connected then
			task.spawn(connection._handler, ...)
		end
	end
end

-- Drops the signals of a destroyed instance (keys start with its id).
function signals.forget(prefix)
	for key in signalsByKey do
		if string.sub(key, 1, #prefix) == prefix then
			signalsByKey[key] = nil
		end
	end
end

local instanceMethods = {}

function instanceMethods.GetPropertyChangedSignal(self, property)
	return signals.get(native.signalKey(self, "prop:" .. property))
end

function instanceMethods.GetAttributeChangedSignal(self, attribute)
	return signals.get(native.signalKey(self, "attr:" .. attribute))
end

-- BindableEvent:Fire. Harmless on other classes, which have no Event signal.
function instanceMethods.Fire(self, ...)
	signals.fire(native.signalKey(self, "Event"), ...)
end

function instanceMethods.WaitForChild(self, name, timeout)
	local child = self:FindFirstChild(name)
	local waited = 0
	local warned = false
	while child == nil do
		if timeout ~= nil and waited >= timeout then
			return nil
		end
		if timeout == nil and not warned and waited >= 5 then
			warned = true
			warn(`Infinite yield possible on '{self:GetFullName()}:WaitForChild("{name}")'`)
		end
		waited += task.wait()
		child = self:FindFirstChild(name)
	end
	return child
end

return {
	waitMarker = MARK,
	instanceMethods = instanceMethods,
	signals = signals,
}
