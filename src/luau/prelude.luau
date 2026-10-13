-- Runs once when the host starts. Receives the Rust natives as its only argument
-- and returns the marker and Luau-implemented methods the host needs.
local native = ...

-- Unique yield tag. A thread that yields (MARK, seconds) is put to sleep by the
-- scheduler; a yield without it is a script's own coroutine and is not ours.
local MARK = {}

function wait(seconds)
	return coroutine.yield(MARK, seconds)
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

local instanceMethods = {}

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
}
