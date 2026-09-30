# ANSI color codes for terminal output
class Colors:
    RESET = '\033[0m'
    RED = '\033[91m'
    GREEN = '\033[92m'
    YELLOW = '\033[93m'
    BLUE = '\033[94m'
    MAGENTA = '\033[95m'
    CYAN = '\033[96m'
    WHITE = '\033[97m'

class Log:
    """Simple logging utility with color support"""
    
    @staticmethod
    def info(message):
        print(f"{Colors.GREEN}{message}{Colors.RESET}")
    
    @staticmethod
    def error(message):
        print(f"{Colors.RED}{message}{Colors.RESET}")
    
    @staticmethod
    def warning(message):
        print(f"{Colors.YELLOW}{message}{Colors.RESET}")
    
    @staticmethod
    def debug(message):
        print(f"{Colors.CYAN}{message}{Colors.RESET}")
    
    @staticmethod
    def blue(message):
        print(f"{Colors.BLUE}{message}{Colors.RESET}")
    
    @staticmethod
    def magenta(message):
        print(f"{Colors.MAGENTA}{message}{Colors.RESET}")
    
    @staticmethod
    def plain(message):
        print(f"{Colors.WHITE}{message}{Colors.RESET}")

log = Log()
