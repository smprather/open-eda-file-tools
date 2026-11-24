# The parent interpreter runs only this prelude. The SDC arrives as text on
# stdin and runs exclusively in a safe child interpreter.
if {[package vcompare [info patchlevel] 8.6] < 0} {
    puts stderr "sdc-tools requires Tcl >= 8.6; found [info patchlevel]"
    exit 3
}

set records {}
proc record {args} { lappend ::records $args }

set slave [interp create -safe]
interp alias $slave record {} record
$slave eval {
    proc unknown {args} { ::record unknown {*}$args; return {} }
    proc collect {kind args} {
        set out {}
        foreach arg $args {
            foreach word $arg { lappend out "$kind:$word" }
        }
        return $out
    }
    proc get_ports {args} { collect port {*}$args }
    proc get_pins {args} { collect pin {*}$args }
    proc get_nets {args} { collect net {*}$args }
    proc get_cells {args} { collect cell {*}$args }
    proc get_clocks {args} { collect clock {*}$args }
    proc create_clock {args} { ::record create_clock {*}$args; return {} }
    proc create_generated_clock {args} { ::record create_generated_clock {*}$args; return {} }
}

set failed [catch {
    fconfigure stdin -encoding utf-8
    set script [read stdin]
    $slave eval $script
} result]

# The protocol uses ASCII counts and length-prefixed UTF-8 strings. Forward
# raw words and any symbolic endpoint lists separately so Rust can consume
# Tcl collections without interpreting Tcl quoting or splitting endpoint names.
fconfigure stdout -translation binary
proc field {text} {
    set bytes [encoding convertto utf-8 $text]
    puts [string length $bytes]
    puts -nonewline $bytes
}
proc endpoints {word} {
    if {[string first : $word] < 0 || [catch {llength $word}]} { return {} }
    foreach endpoint $word {
        if {![regexp {^(port|pin|net|cell):} $endpoint]} { return {} }
    }
    return $word
}

if {$failed} {
    puts error
    field $result
} else {
    puts ok
}
puts [llength $records]
foreach row $records {
    puts [llength $row]
    foreach word $row {
        field $word
        set members [endpoints $word]
        puts [llength $members]
        foreach member $members { field $member }
    }
}
exit 0
