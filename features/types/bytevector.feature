Feature: Bytevector

  Scenario: Write a bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (write-bytevector #u8(65 66 67))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "ABC"

  Scenario Outline: Get a length of a bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (write-u8 (if (= (bytevector-length <value>) <length>) 65 66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

    Examples:
      | value      | length |
      | #u8()      | 0      |
      | #u8(0)     | 1      |
      | #u8(0 0)   | 2      |
      | #u8(0 0 0) | 3      |

  Scenario Outline: Reference a value in a bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (write-u8 (bytevector-u8-ref <vector> <index>))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

    Examples:
      | vector        | index |
      | #u8(65)       | 0     |
      | #u8(65 66)    | 0     |
      | #u8(66 65)    | 1     |
      | #u8(65 66 66) | 0     |
      | #u8(66 65 66) | 1     |
      | #u8(66 66 65) | 2     |

  Scenario Outline: Set a value in a bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define xs (bytevector <values>))

      (bytevector-u8-set! xs <index> <value>)

      (write-u8 (if (equal? xs #u8(<result>)) 65 66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

    Examples:
      | values | index | value | result |
      | 0      | 0     | 1     | 1      |
      | 0 1    | 0     | 2     | 2 1    |
      | 0 1    | 1     | 2     | 0 2    |
      | 0 1 2  | 0     | 3     | 3 1 2  |
      | 0 1 2  | 1     | 3     | 0 3 2  |
      | 0 1 2  | 2     | 3     | 0 1 3  |

  Scenario Outline: Append bytevectors
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base) (srfi 1))

      (define xs (bytevector-append <values>))

      (for-each
        (lambda (index)
          (write-u8 (bytevector-u8-ref xs index)))
        (iota (bytevector-length xs)))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "<output>"

    Examples:
      | values                           | output |
      | #u8()                            |        |
      | #u8() #u8()                      |        |
      | #u8(65)                          | A      |
      | #u8(65) #u8(66)                  | AB     |
      | #u8(65) #u8(66) #u8(67)          | ABC    |
      | #u8(65) #u8(66 67) #u8(68 69 70) | ABCDEF |

  Scenario Outline: Copy a bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base) (srfi 1))

      (define xs (bytevector-copy <value>))

      (for-each
        (lambda (index)
          (write-u8 (bytevector-u8-ref xs index)))
        (iota (bytevector-length xs)))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "<output>"

    Examples:
      | value         | output |
      | #u8()         |        |
      | #u8(65)       | A      |
      | #u8(65 66)    | AB     |
      | #u8(65 66 67) | ABC    |

  Scenario Outline: Copy a bytevector in place
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base) (srfi 1))

      (define xs (bytevector <values>))

      (bytevector-copy! xs <arguments>)

      (for-each
        (lambda (index)
          (write-u8 (+ 65 (bytevector-u8-ref xs index))))
        (iota (bytevector-length xs)))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "<output>"

    # spell-checker: disable
    Examples:
      | values    | arguments          | output |
      |           | 0 #u8()            |        |
      | 0 1 2     | 0 #u8(3 4 5)       | DEF    |
      | 0 1 2     | 1 #u8(3 4)         | ADE    |
      | 0 1 2     | 2 #u8(3)           | ABD    |
      | 0 1 2 3 4 | 1 #u8(5 6 7)       | AFGHE  |
      | 0 1 2 3   | 1 #u8(4 5 6 7) 1   | AFGH   |
      | 0 1 2 3   | 1 #u8(4 5 6 7) 1 3 | AFGD   |

  Scenario: Preserve temporary-storage overlap semantics when copying a bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define xs (bytevector 0 1 2 3))
      (bytevector-copy! xs 1 xs 0 3)
      (write-u8 (+ 65 (bytevector-u8-ref xs 0)))
      (write-u8 (+ 65 (bytevector-u8-ref xs 1)))
      (write-u8 (+ 65 (bytevector-u8-ref xs 2)))
      (write-u8 (+ 65 (bytevector-u8-ref xs 3)))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "AABC"

  Scenario: Preserve left overlap semantics when copying a bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define xs (bytevector 0 1 2 3))
      (bytevector-copy! xs 0 xs 1 4)
      (write-u8 (+ 65 (bytevector-u8-ref xs 0)))
      (write-u8 (+ 65 (bytevector-u8-ref xs 1)))
      (write-u8 (+ 65 (bytevector-u8-ref xs 2)))
      (write-u8 (+ 65 (bytevector-u8-ref xs 3)))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "BCDD"

  Scenario: Make an empty bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (write-u8 (if (= (bytevector-length (make-bytevector 0))) 65 66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

  Scenario Outline: Make and mutate a filled bytevector
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (define xs (make-bytevector <length> <fill>))

      (bytevector-u8-set! xs <index> 65)

      (write-u8
        (if (and (= (bytevector-length xs) <length>)
                 (= (bytevector-u8-ref xs <index>) 65)
                 (= (bytevector-u8-ref xs 0) <first>))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

    Examples:
      | length | fill | index | first |
      | 1      | 0    | 0     | 65    |
      | 63     | 37   | 62    | 37    |
      | 64     | 37   | 63    | 37    |
      | 65     | 255  | 64    | 255   |
      | 4096   | 37   | 4095  | 37    |
      | 4097   | 255  | 4096  | 255   |

  Scenario Outline: Reject nonnumeric make-bytevector arguments
    Given a file named "main.scm" with:
      """scheme
      (import (scheme base))

      (write-u8
        (if (guard
              (condition (else #t))
              (begin (make-bytevector <length> <fill>) #f))
            65
            66))
      """
    When I successfully run `stak main.scm`
    Then the stdout should contain exactly "A"

    Examples:
      | length | fill |
      | #f     | 0    |
      | 1      | #f   |

  @long
  Rule: Large bytevector

    Scenario Outline: Reference an element
      Given a file named "main.scm" with:
        """scheme
        (import (scheme base))

        (write-u8 (bytevector-u8-ref (make-bytevector <length> 65) <index>))
        """
      When I successfully run `stak main.scm`
      Then the stdout should contain exactly "A"

      Examples:
        | length | index |
        | 1      | 0     |
        | 2      | 0     |
        | 2      | 1     |
        | 3      | 0     |
        | 3      | 1     |
        | 3      | 2     |
        | 8      | 0     |
        | 8      | 1     |
        | 8      | 6     |
        | 8      | 7     |
        | 9      | 0     |
        | 9      | 1     |
        | 9      | 7     |
        | 9      | 8     |
        | 64     | 63    |
        | 65     | 64    |
        | 512    | 511   |
        | 513    | 512   |
        | 4096   | 4095  |
        | 4097   | 4096  |

    @gauche @guile @stak
    Scenario Outline: Use a bytevector literal
      Given a file named "main.scm" with:
        """scheme
        (import (scheme base))

        (define xs (include "./value.scm"))

        (write-u8 (if (= (bytevector-u8-ref xs <index>) <index>) 65 66))
        """
      And a file named "write.scm" with:
        """scheme
        (import (scheme base) (scheme write) (srfi 1))

        (write
          (apply
            bytevector
            (map
              (lambda (x) (remainder x 256))
              (iota <length>))))
        """
      And I run the following script:
        """sh
        stak write.scm > value.scm
        """
      When I successfully run `stak main.scm`
      Then the stdout should contain exactly "A"

      Examples:
        | length | index |
        | 1      | 0     |
        | 2      | 0     |
        | 2      | 1     |
        | 512    | 0     |
        | 512    | 1     |
        | 512    | 254   |
        | 512    | 255   |
        | 4096   | 0     |
        | 4096   | 1     |
        | 4096   | 254   |
        | 4096   | 255   |
        | 8192   | 0     |
        | 8192   | 1     |
        | 8192   | 254   |
        | 8192   | 255   |
