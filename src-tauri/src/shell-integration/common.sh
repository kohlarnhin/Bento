_bento_cwd() {
  local LC_ALL=C directory=$PWD encoded='' c escape i
  for (( i=0; i<${#directory}; i++ )); do
    c=${directory:$i:1}
    case $c in
      [a-zA-Z0-9_./~-]) encoded+=$c ;;
      *) printf -v escape '%%%02X' "'$c"; encoded+=$escape ;;
    esac
  done
  printf '\033]7;file://localhost%s\007' "$encoded"
}

_bento_command_start() {
  local command=${1//$'\033'/}
  command=${command//$'\007'/}
  printf '\033]133;B\007\033]133;C;%s\007' "$command"
}

_bento_command_end() {
  printf '\033]133;D;%d\007' "$1"
}

_bento_prompt() {
  _bento_cwd
  printf '\033]133;A\007'
}
