# Eludite's shell integration for fish (brief 0041), sourced through `--init-command` after your own config files
# (nothing in your home folder is changed): OSC 133 marks around prompts and commands (A prompt start, B command
# start, C output start, D;<exit code> command end) and the folder (OSC 7).
if status is-interactive; and not set -q __ELUDITE_INTEGRATED
    set -g __ELUDITE_INTEGRATED 1
    set -g __eludite_ran 0
    function __eludite_preexec --on-event fish_preexec
        set -g __eludite_ran 1
        printf '\e]133;C\a'
    end
    function __eludite_postexec --on-event fish_postexec
        set -l s $status
        if test "$__eludite_ran" = 1
            printf '\e]133;D;%s\a' $s
            set -g __eludite_ran 0
        end
    end
    functions -c fish_prompt __eludite_user_prompt
    function fish_prompt
        printf '\e]133;A\a\e]7;file://%s%s\a' (prompt_hostname) $PWD
        __eludite_user_prompt
        printf '\e]133;B\a'
    end
end
