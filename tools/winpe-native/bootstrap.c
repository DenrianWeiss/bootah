#ifndef UNICODE
#define UNICODE
#endif
#ifndef _UNICODE
#define _UNICODE
#endif
#include <windows.h>
#include <winhttp.h>
#include <shellapi.h>

static void message(const char *text) {
    DWORD written;
    WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), text, (DWORD)lstrlenA(text), &written, NULL);
}

#ifdef BOOTAH_NO_CRT
void *memset(void *ptr, int value, size_t size) {
    volatile unsigned char *out = ptr;
    while (size--) *out++ = (unsigned char)value;
    return ptr;
}
#endif

int wmain(int argc, wchar_t **argv) {
    if (argc != 3) return 2;
    HANDLE token;
    if (OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &token)) {
        TOKEN_PRIVILEGES p = {0}; p.PrivilegeCount = 1;
        if (LookupPrivilegeValueW(NULL, SE_SYSTEM_ENVIRONMENT_NAME, &p.Privileges[0].Luid)) {
            p.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
            AdjustTokenPrivileges(token, FALSE, &p, 0, NULL, NULL);
        }
        CloseHandle(token);
    }
    BYTE secure = 0, setup = 1;
    const wchar_t *guid = L"{8BE4DF61-93CA-11D2-AA0D-00E098032B8C}";
    if (GetFirmwareEnvironmentVariableW(L"SecureBoot", guid, &secure, 1) == 1 &&
        GetFirmwareEnvironmentVariableW(L"SetupMode", guid, &setup, 1) == 1)
        message(secure == 1 && setup == 0 ? "Bootah firmware: SecureBoot=1 SetupMode=0\r\n" : "Bootah firmware: Secure Boot is not active\r\n");
    else message("Bootah firmware state unavailable\r\n");
    URL_COMPONENTS url = {0}; url.dwStructSize = sizeof(url);
    static wchar_t host[256], path[2048];
    url.lpszHostName = host; url.dwHostNameLength = 256;
    url.lpszUrlPath = path; url.dwUrlPathLength = 2048;
    if (!WinHttpCrackUrl(argv[1], 0, 0, &url) || url.nScheme != INTERNET_SCHEME_HTTP) return 3;
    HINTERNET session = WinHttpOpen(L"Bootah native WinPE", WINHTTP_ACCESS_TYPE_NO_PROXY, NULL, NULL, 0);
    if (!session) return 4;
    WinHttpSetTimeouts(session, 10000, 10000, 30000, 30000);
    HINTERNET connection = WinHttpConnect(session, host, url.nPort, 0);
    HINTERNET request = connection ? WinHttpOpenRequest(connection, L"GET", path, NULL, NULL, NULL, 0) : NULL;
    int result = 5;
    HANDLE output = INVALID_HANDLE_VALUE;
    if (!request) goto done;
    DWORD redirects = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
    WinHttpSetOption(request, WINHTTP_OPTION_REDIRECT_POLICY, &redirects, sizeof(redirects));
    if (!WinHttpSendRequest(request, NULL, 0, NULL, 0, 0, 0) || !WinHttpReceiveResponse(request, NULL)) goto done;
    DWORD status = 0, size = sizeof(status);
    if (!WinHttpQueryHeaders(request, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, NULL, &status, &size, NULL) || status != 200) {
        message("Bootah: server rejected startup request\r\n"); goto done;
    }
    output = CreateFileW(argv[2], GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (output == INVALID_HANDLE_VALUE) goto done;
    DWORD total = 0;
    for (;;) {
        static BYTE buffer[8192]; DWORD received = 0, written = 0;
        if (!WinHttpReadData(request, buffer, sizeof(buffer), &received)) goto done;
        if (!received) break;
        total += received;
        if (total > 1024 * 1024 || !WriteFile(output, buffer, received, &written, NULL) || written != received) goto done;
    }
    result = total ? 0 : 6;
done:
    if (result) message("Bootah: startup download failed\r\n");
    if (output != INVALID_HANDLE_VALUE) CloseHandle(output);
    if (request) WinHttpCloseHandle(request);
    if (connection) WinHttpCloseHandle(connection);
    WinHttpCloseHandle(session);
    if (result) DeleteFileW(argv[2]);
    return result;
}

#ifdef BOOTAH_NO_CRT
void mainCRTStartup(void) {
    int argc = 0;
    wchar_t **argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    int result = argv ? wmain(argc, argv) : 2;
    if (argv) LocalFree(argv);
    ExitProcess((UINT)result);
}
#endif
