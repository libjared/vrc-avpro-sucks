should_show_seconds = false

function seconds_hook()
    local time_pos = mp.get_property_number("time-pos")

    if not should_show_seconds then
        return
    end

    -- time_pos can be nil when closing
    if time_pos ~= nil then
        mp.osd_message(string.format("%.2f", time_pos))
    else
        mp.osd_message("N/A")
    end
end

mp.register_event("tick", seconds_hook)

-- press semicolon to toggle seconds display
mp.add_key_binding(";", "toggle_seconds", function()
    should_show_seconds = not should_show_seconds
    if should_show_seconds then
        mp.osd_message("Seconds: ON")
    else
        mp.osd_message("Seconds: OFF")
    end
end)

-- A bash one-liner that displays the seconds since a target time.
-- target="6:07:29 pm"; watch -n0.5 'echo $(($(date +%s) - $(date -d "'"$target"'" +%s)))'
