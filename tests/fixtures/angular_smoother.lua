local abs = math.abs
local floor = math.floor
local pi = math.pi
local tau = pi*2
local inv_tau = 1/tau

local last_y = 0
local snap = 0.0001

function process(frames)
    -- input angle from touch processing is expected to be [0, tau)
    -- output samples are signed-wrapped to [-pi, pi) for sin/cos trajectory use.
    
    -- shadow hot globals/upvalues with locals:
    local floor, pi, tau, inv_tau, input, output = floor, pi, tau, inv_tau, input, output
    local a = 1 - alpha[1]
    local y = last_y
    
    for i = 1, frames do
        local d = input[i] - y
        -- shortest signed angular delta
        d = d - tau * floor((d + pi) * inv_tau)
        y = y + a * d
        y = y - tau * floor((y + pi) * inv_tau)
        output[i] = y
    end
    
    local x = input[frames]
    local d = x - y
    d = d - tau * floor((d + pi) * inv_tau)
    if abs(d) <= snap then
        y = x
        y = y - tau * floor((y + pi) * inv_tau)
    end
    
    last_y = y
end
