Feature: Port

  Scenario Outline: Check if a value is a port
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (write-u8 (if (port? <expression>) 65 66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

    Examples:
      | expression            |
      | (current-input-port)  |
      | (current-output-port) |

  Scenario: Check if a value is an input port
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (write-u8 (if (input-port? (current-input-port)) 65 66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario Outline: Check if a value is an output port
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (write-u8 (if (output-port? <expression>) 65 66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

    Examples:
      | expression            |
      | (current-output-port) |
      | (current-error-port)  |

  Scenario Outline: Check if an input port is open or not
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base) (scheme file))

      (define port <expression>)

      (write-u8 (if (input-port-open? port) 65 66))
      (close-input-port port)
      (write-u8 (if (input-port-open? port) 65 66))
      """
    And a file named "foo.txt" with:
      """
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "AB"

    Examples:
      | expression                  |
      | (open-input-file "foo.txt") |
      | (open-input-string "foo")   |

  Scenario Outline: Check if an output port is open or not
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base) (scheme file))

      (define port <expression>)

      (write-u8 (if (output-port-open? port) 65 66))
      (close-output-port port)
      (write-u8 (if (output-port-open? port) 65 66))
      """
    And a file named "foo.txt" with:
      """
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "AB"

    Examples:
      | expression                   |
      | (open-output-file "foo.txt") |
      | (open-output-string)         |

  @gauche @guile @stak
  Scenario Outline: Preserve pending input order after peeking
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define port (open-input-string "<input>"))
      (peek-char port)
      (peek-u8 port)
      (write-u8 (if (= (read-u8 port) <first-byte>) 65 66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

    Examples:
      | input | first-byte |
      | éx    | 195        |
      | 😄x    | 240        |

  @stak
  Scenario: Preserve pending input order when peeking a character
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base) (stak io))

      (define port
        (make-port
          (lambda () #f)
          #f
          #f
          (lambda () #f)
          '(195 169 120)))

      (peek-char port)

      (for-each
        (lambda (expected)
          (write-u8 (if (= (read-u8 port) expected) 65 66)))
        '(195 169 120))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "AAA"

  @gauche @guile @stak
  Scenario Outline: Read from a string port
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (call-with-port
        (open-input-string "<string>")
        (lambda (port)
          (parameterize ((current-input-port port))
            (do ((x (read-u8) (read-u8)))
              ((eof-object? x) #f)
              (write-u8 x)))))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "<string>"

    Examples:
      | string |
      | ABC    |
      | あ      |
      | 😄      |
      | —      |

  @gauche @guile @stak
  Scenario Outline: Write to a string port
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (call-with-port
        (open-output-string)
        (lambda (port)
          (parameterize ((current-output-port port))
            (for-each write-u8 '(<bytes>)))
          (let ((xs (get-output-string port)))
            (unless (= (string-length xs) <length>)
              (error "invalid length"))
            (write-string xs))))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "<output>"

    Examples:
      | bytes           | output | length |
      | 65 66 67        | ABC    | 3      |
      | 227 129 130     | あ      | 1      |
      | 240 159 152 132 | 😄      | 1      |

  Scenario: Read from a bytevector port
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (call-with-port
        (open-input-bytevector #u8(65 66 67))
        (lambda (port)
          (parameterize ((current-input-port port))
            (do ((x (read-u8) (read-u8)))
              ((eof-object? x) #f)
              (write-u8 x)))))
      """
    When I successfully run `stak main.scm`

  Scenario Outline: Write to a bytevector port
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (call-with-port
        (open-output-bytevector)
        (lambda (port)
          (parameterize ((current-output-port port))
            (for-each write-u8 '(<bytes>)))
          (let ((xs (get-output-bytevector port)))
            (unless (= (bytevector-length xs) <length>)
              (error "invalid length"))
            (write-bytevector xs))))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "<output>"

    Examples:
      | bytes    | output | length |
      | 65       | A      | 1      |
      | 65 66    | AB     | 2      |
      | 65 66 67 | ABC    | 3      |

  Scenario: Preserve chunked bytevector output across retrievals
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define port (open-output-bytevector))

      (do ((index 0 (+ index 1)))
        ((= index 65))
        (write-u8 index port))

      (define first (get-output-bytevector port))
      (define first-length (bytevector-length first))
      (define first-at-zero (bytevector-u8-ref first 0))
      (define first-at-63 (bytevector-u8-ref first 63))
      (define first-at-64 (bytevector-u8-ref first 64))
      (define second (get-output-bytevector port))
      (define same-output? (equal? first second))

      (write-u8 65 port)
      (define third (get-output-bytevector port))

      (write-u8
        (if (and (= first-length 65)
                 (= first-at-zero 0)
                 (= first-at-63 63)
                 (= first-at-64 64)
                 same-output?
                 (= (bytevector-length third) 66)
                 (= (bytevector-u8-ref third 65) 65))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario: Read a bytevector port across the chunk boundary
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define source (make-bytevector 65 255))
      (bytevector-u8-set! source 0 0)
      (bytevector-u8-set! source 63 63)
      (bytevector-u8-set! source 64 64)

      (define port (open-input-bytevector source))
      (define at-zero (read-u8 port))
      (do ((index 1 (+ index 1)))
        ((= index 63))
        (read-u8 port))
      (define at-63 (read-u8 port))
      (define at-64 (read-u8 port))
      (define eof (read-u8 port))
      (define eof-again (read-u8 port))

      (write-u8
        (if (and (= at-zero 0)
                 (= at-63 63)
                 (= at-64 64)
                 (eof-object? eof)
                 (eof-object? eof-again))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario: Preserve string output across the chunk boundary
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define port (open-output-string))
      (do ((index 0 (+ index 1)))
        ((= index 65))
        (write-u8 65 port))

      (define first (get-output-string port))
      (define first-length (string-length first))
      (define first-at-zero (char->integer (string-ref first 0)))
      (define first-at-63 (char->integer (string-ref first 63)))
      (define first-at-64 (char->integer (string-ref first 64)))
      (define second (get-output-string port))
      (define same-output? (equal? first second))

      (write-u8 66 port)
      (define third (get-output-string port))

      (write-u8
        (if (and (= first-length 65)
                 (= first-at-zero 65)
                 (= first-at-63 65)
                 (= first-at-64 65)
                 same-output?
                 (= (string-length third) 66)
                 (= (char->integer (string-ref third 65)) 66))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario: Read UTF-8 string input across a long source
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define source
        (string-append "A" (make-string 62 #\\a) "あ😄"))

      (define port (open-input-string source))
      (define at-zero (read-u8 port))
      (do ((index 1 (+ index 1)))
        ((= index 63))
        (read-u8 port))
      (define at-63-a (read-u8 port))
      (define at-63-b (read-u8 port))
      (define at-63-c (read-u8 port))
      (define at-64-a (read-u8 port))
      (define at-64-b (read-u8 port))
      (define at-64-c (read-u8 port))
      (define at-64-d (read-u8 port))
      (define eof (read-u8 port))

      (write-u8
        (if (and (= at-zero 65)
                 (= at-63-a 227)
                 (= at-63-b 129)
                 (= at-63-c 130)
                 (= at-64-a 240)
                 (= at-64-b 159)
                 (= at-64-c 152)
                 (= at-64-d 132)
                 (eof-object? eof))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario: Retrieve string output incrementally without rescanning
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define port (open-output-string))
      (do ((index 0 (+ index 1)))
        ((= index 1000))
        (write-u8 65 port)
        (get-output-string port))

      (define result (get-output-string port))
      (write-u8
        (if (and (= (string-length result) 1000)
                 (= (char->integer (string-ref result 0)) 65)
                 (= (char->integer (string-ref result 999)) 65))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario: Stop reading after an invalid UTF-8 continuation
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define reads 0)
      (define source '(226 65))
      (define port
        (make-input-port
          (lambda ()
            (set! reads (+ reads 1))
            (if (pair? source)
                (let ((byte (car source)))
                  (set! source (cdr source))
                  byte)
                (error "read past known-invalid UTF-8")))
          (lambda () #f)))

      (define replacement (read-char port))
      (define ascii (read-char port))
      (write-u8
        (if (and (= (char->integer replacement) 65533)
                 (char=? ascii #\A)
                 (= reads 2))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario: Preserve UTF-8 decoding across output retrieval
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define port (open-output-string))
      (write-u8 226 port)
      (define first (get-output-string port))
      (define first-length (string-length first))
      (write-u8 130 port)
      (define second (get-output-string port))
      (define second-length (string-length second))
      (write-u8 172 port)
      (define final (get-output-string port))
      (define final-length (string-length final))

      (write-u8
        (if (and (= first-length 0)
                 (= second-length 0)
                 (= final-length 1)
                 (= (char->integer (string-ref final 0)) 8364))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario: Repeat peek-char on truncated UTF-8
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define port (open-input-bytevector (list->bytevector '(226 130))))
      (define first (peek-char port))
      (define second (peek-char port))
      (define third (read-char port))

      (write-u8
        (if (and (= (char->integer first) 65533)
                 (= (char->integer second) 65533)
                 (= (char->integer third) 65533))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario: Retrieve multibyte output incrementally without rescanning
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define port (open-output-string))
      (do ((index 0 (+ index 1)))
        ((= index 1000))
        (write-u8 227 port)
        (get-output-string port)
        (write-u8 129 port)
        (get-output-string port)
        (write-u8 130 port)
        (get-output-string port))

      (define result (get-output-string port))
      (write-u8
        (if (and (= (string-length result) 1000)
                 (= (char->integer (string-ref result 0)) 12354)
                 (= (char->integer (string-ref result 999)) 12354))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"
